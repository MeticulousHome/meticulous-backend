"""Render a manual profile document as the node program the ESP32 runs.

An ordinary profile describes a curve the machine follows, so the converter can
turn its stages into nodes that drive a controller along that curve. A manual
profile has no curve: the encoder sets the target live, and the shot ends on a
long press, or on the backend's `finish` -- the dial sends it on a double click
inside a manual stage (contract sections 3 and 8). The document therefore only
carries the *shape* of the session -- which control the shot starts in, and the
temperature and weight that bound it -- and the nodes below implement the
interaction itself.

The head (prepare, purge, water detection, heating, click to start, retracting,
closing valve) and the tail (retracting, purge or remove cup, END_STAGE) come
from `profile_converter` unchanged; only the user stages in between are ours.

Implements section 8 ("Node program") of the Manual mode cross-repo contract.
"""

from config import (
    CONFIG_USER,
    MAX_PISTON_POSITION,
    MeticulousConfig,
    PROFILE_AUTO_PURGE,
)
from log import MeticulousLogger
from profile_converter import simplified_json
from profile_converter.dictionaries import algorithms_type, source_type
from profile_converter.enums import (
    AlgorithmType,
    ButtonGestureSourceType,
    ButtonSourceType,
    FlowAlgorithmType,
    PressureAlgorithmType,
    SourceType,
    TriggerOperatorType,
)
from profile_converter.nodes import Nodes
from profile_converter.profile_converter import ComplexProfileConverter
from profile_converter.simplified_json import InitNode
from profile_converter.triggers import (
    ButtonTrigger,
    PistonPositionTrigger,
    UserFinishTrigger,
    WeightTrigger,
)

logger = MeticulousLogger.getLogger(__name__)

# The head's "closing valve" node exits to `end_node_head` and the tail's first
# node *is* `init_node_tail`, so these two numbers place the manual stages
# between the templates. They are the pair the converter's own example uses.
END_NODE_HEAD = 1000
INIT_NODE_TAIL = 7000

# The id of the ESP's end-of-profile node. It is a real node in the tail, but a
# trigger may name it before that node has been collected, so it is spelled out.
END_STAGE_NODE_ID = -2

# One detent of the encoder moves the target by `step`, clamped to [min, max].
MANUAL_CONTROLLER_STEP = 0.1
MANUAL_CONTROLLER_MIN = 0.0
MANUAL_CONTROLLER_MAX = 12.0

# Re-entering a stage picks the target up from the sensor rather than from zero,
# so the machine does not lurch; the gain leans the target slightly above the
# reading it resumes from.
MANUAL_RESUME_SENSOR_GAIN = 1.1

# The converter renders a piston-position exit trigger as a percentage of the
# usable travel, `(percent / 100) * (MAX_PISTON_POSITION - 2)`. A manual shot
# ends when the piston reaches 100 % of that travel.
MANUAL_PISTON_LIMIT = MAX_PISTON_POSITION - 2

# The weight and position references the head establishes for the whole shot.
# The manual triggers stop the shot against those, not against per-stage ones,
# because the user can hop between the stages any number of times.
SHOT_WEIGHT_REFERENCE_ID = 1
SHOT_POSITION_REFERENCE_ID = 0

_MANUAL_CONTROLLERS = {
    "pressure": {
        "kind": "manual_pressure_controller",
        "algorithm": algorithms_type[AlgorithmType.PRESSURE][PressureAlgorithmType.PID_V1],
        "sensor": source_type[SourceType.RAW][SourceType.PRESSURE],
        "stage_name": "Manual pressure",
    },
    "flow": {
        "kind": "manual_flow_controller",
        "algorithm": algorithms_type[AlgorithmType.FLOW][FlowAlgorithmType.PID_V1],
        "sensor": source_type[SourceType.RAW][SourceType.FLOW],
        "stage_name": "Manual flow",
    },
}

MANUAL_CONTROLLER_KINDS = frozenset(spec["kind"] for spec in _MANUAL_CONTROLLERS.values())


class _RawController:
    """Adapts a plain controller dict to the converter's `add_controller` API.

    The manual controllers are new kinds the ESP parses. `profile_converter`'s
    `controllers.py` does not model them and is not ours to extend, so they are
    built as dicts and wrapped to satisfy `Nodes.add_controller`.
    """

    def __init__(self, data: dict):
        self.data = data

    def get_controller(self) -> dict:
        return self.data


def _manual_controller(stage_type: str, initial: dict) -> _RawController:
    """The manual controller of `stage_type`, starting from `initial`."""
    spec = _MANUAL_CONTROLLERS[stage_type]
    return _RawController(
        {
            "kind": spec["kind"],
            "algorithm": spec["algorithm"],
            "step": MANUAL_CONTROLLER_STEP,
            "min": MANUAL_CONTROLLER_MIN,
            "max": MANUAL_CONTROLLER_MAX,
            "initial": initial,
        }
    )


def _manual_triggers(final_weight: float, other_resume_node_id: int) -> list:
    """The five exits every manual node carries, in first-match-wins order.

    A fresh set per node: the converter's trigger objects hand out the dict they
    hold, so sharing one would let a later edit reach into an earlier node.
    """
    return [
        ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON,
            ButtonGestureSourceType.SINGLE,
            other_resume_node_id,
        ),
        ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON,
            ButtonGestureSourceType.LONG,
            INIT_NODE_TAIL,
        ),
        UserFinishTrigger(INIT_NODE_TAIL),
        WeightTrigger(
            SourceType.PREDICTIVE,
            TriggerOperatorType.GREATER_THAN_OR_EQUAL,
            final_weight,
            SHOT_WEIGHT_REFERENCE_ID,
            INIT_NODE_TAIL,
        ),
        PistonPositionTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL,
            MANUAL_PISTON_LIMIT,
            SHOT_POSITION_REFERENCE_ID,
            INIT_NODE_TAIL,
        ),
    ]


def _initial_target(stage: dict) -> float:
    """The target the shot starts at: the first point of the stage's dynamics."""
    points = (stage.get("dynamics") or {}).get("points")
    try:
        value = points[0][1]
    except (IndexError, KeyError, TypeError):
        raise ValueError("a manual stage needs a first dynamics point to start from")
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"the initial manual target must be a number, got {value!r}")
    return float(value)


def _stage_name(stage: dict, stage_type: str) -> str:
    """The name the ESP reports while the stage runs."""
    name = stage.get("name")
    if isinstance(name, str) and name.strip():
        return name
    return _MANUAL_CONTROLLERS[stage_type]["stage_name"]


def manual_stages(profile: dict, converter: ComplexProfileConverter) -> list:
    """The stages that replace the converted user stages of a manual profile.

    Each document stage becomes an `init` node holding the stage's own time,
    weight and position references, and a `resume` node driving the manual
    controller. The document's first stage also gets a `start` node, entered
    once from the head, whose target begins at the document's value instead of
    at the sensor reading.

    Node ids come from the converter's allocator, seeded at `END_NODE_HEAD` the
    way `SimplifiedJson.to_complex` seeds it, so the first stage's `init` node
    is the node the head's "closing valve" already exits to.
    """
    stages = profile.get("stages")
    if not isinstance(stages, list) or not stages:
        raise ValueError("a manual profile needs at least one stage")

    allocator = converter.complex
    simplified_json.current_node_id = END_NODE_HEAD
    final_weight = allocator.get_final_weight()

    plans = []
    for index, stage in enumerate(stages):
        stage_type = stage.get("type")
        if stage_type not in _MANUAL_CONTROLLERS:
            raise ValueError(f"unsupported manual stage type {stage_type!r}")
        plans.append(
            {
                "stage": stage,
                "type": stage_type,
                "init_id": allocator.get_new_node_id(),
                "start_id": allocator.get_new_node_id() if index == 0 else None,
                "resume_id": allocator.get_new_node_id(),
            }
        )

    program_stages = []
    for index, plan in enumerate(plans):
        entry_id = plan["start_id"] if plan["start_id"] is not None else plan["resume_id"]

        init_node = InitNode(plan["init_id"])
        init_node.set_time_id(allocator.get_new_reference_id())
        init_node.set_weight_id(allocator.get_new_reference_id())
        init_node.set_position_id(allocator.get_new_reference_id())
        init_node.set_next_node_id(entry_id)

        # A tap hands the shot to the next stage and wraps, which for the two
        # stages of section 2 is exactly "the other stage".
        other_resume_id = plans[(index + 1) % len(plans)]["resume_id"]
        nodes = [init_node.get_node()]

        if plan["start_id"] is not None:
            start_node = Nodes(plan["start_id"])
            start_node.add_controller(
                _manual_controller(
                    plan["type"],
                    {"kind": "value", "value": _initial_target(plan["stage"])},
                )
            )
            for trigger in _manual_triggers(final_weight, other_resume_id):
                start_node.add_trigger(trigger)
            nodes.append(start_node.get_node())

        resume_node = Nodes(plan["resume_id"])
        resume_node.add_controller(
            _manual_controller(
                plan["type"],
                {
                    "kind": "sensor",
                    "source": _MANUAL_CONTROLLERS[plan["type"]]["sensor"],
                    "gain": MANUAL_RESUME_SENSOR_GAIN,
                },
            )
        )
        for trigger in _manual_triggers(final_weight, other_resume_id):
            resume_node.add_trigger(trigger)
        nodes.append(resume_node.get_node())

        program_stages.append(
            {"name": _stage_name(plan["stage"], plan["type"]), "nodes": nodes}
        )

    return program_stages


def _is_manual_stage(stage: dict) -> bool:
    return any(
        controller.get("kind") in MANUAL_CONTROLLER_KINDS
        for node in stage.get("nodes") or []
        for controller in node.get("controllers") or []
    )


def validate_program(program: dict) -> None:  # noqa: C901
    """Raise `ValueError` unless the node program is one the ESP can run.

    The ESP follows `next_node_id` without checking it, so a dangling id strands
    the machine mid-shot with the piston loaded. These are the invariants the
    manual path can break on its own; the head and tail are template output.
    """
    stages = program.get("stages")
    if not isinstance(stages, list) or not stages:
        raise ValueError("the node program has no stages")

    manual_indexes = [index for index, stage in enumerate(stages) if _is_manual_stage(stage)]
    if not manual_indexes:
        raise ValueError("the node program has no manual stages")

    head = stages[: manual_indexes[0]]
    manual = stages[manual_indexes[0] : manual_indexes[-1] + 1]
    tail = stages[manual_indexes[-1] + 1 :]

    def node_ids(group) -> list:
        return [node["id"] for stage in group for node in stage.get("nodes") or []]

    manual_ids = node_ids(manual)
    template_ids = node_ids(head) + node_ids(tail)

    collisions = sorted(set(manual_ids) & set(template_ids))
    if collisions:
        raise ValueError(f"manual node ids collide with the head or tail: {collisions}")

    all_ids = manual_ids + template_ids
    duplicates = sorted({node_id for node_id in all_ids if all_ids.count(node_id) > 1})
    if duplicates:
        raise ValueError(f"duplicate node ids in the node program: {duplicates}")

    if not tail or not (tail[0].get("nodes") or []):
        raise ValueError("the node program has no tail")
    tail_entry = tail[0]["nodes"][0]["id"]
    if tail_entry != INIT_NODE_TAIL:
        raise ValueError(f"the tail is entered at {tail_entry}, expected {INIT_NODE_TAIL}")

    known_ids = set(all_ids)
    for stage in stages:
        for node in stage.get("nodes") or []:
            for trigger in node.get("triggers") or []:
                target = trigger.get("next_node_id")
                if target != END_STAGE_NODE_ID and target not in known_ids:
                    raise ValueError(
                        f"node {node['id']} of stage {stage.get('name')!r} points at "
                        f"unknown node {target}"
                    )

    head_ids = set(node_ids(head))
    head_exits = {
        trigger.get("next_node_id")
        for stage in head
        for node in stage.get("nodes") or []
        for trigger in node.get("triggers") or []
        if trigger.get("next_node_id") not in head_ids
        and trigger.get("next_node_id") != END_STAGE_NODE_ID
    }
    if head_exits != {END_NODE_HEAD}:
        raise ValueError(f"the head exits to {sorted(head_exits)}, expected [{END_NODE_HEAD}]")

    start_ids = [
        node["id"]
        for node in manual[0].get("nodes") or []
        for controller in node.get("controllers") or []
        if controller.get("kind") in MANUAL_CONTROLLER_KINDS
        and (controller.get("initial") or {}).get("kind") == "value"
    ]
    if len(start_ids) != 1:
        raise ValueError(
            f"the first manual stage needs exactly one start node, found {len(start_ids)}"
        )

    triggers_by_id = {
        node["id"]: [trigger.get("next_node_id") for trigger in node.get("triggers") or []]
        for stage in stages
        for node in stage.get("nodes") or []
    }
    reached = set()
    pending = [END_NODE_HEAD]
    while pending:
        node_id = pending.pop()
        if node_id in reached or node_id not in triggers_by_id:
            continue
        reached.add(node_id)
        pending.extend(triggers_by_id[node_id])

    if start_ids[0] not in reached:
        raise ValueError(
            f"the first manual stage's start node {start_ids[0]} is unreachable "
            f"from the head exit {END_NODE_HEAD}"
        )


def build_manual_program(profile: dict) -> dict:
    """The node program for a manual profile document (contract section 8)."""
    click_to_purge = not bool(MeticulousConfig[CONFIG_USER][PROFILE_AUTO_PURGE])
    converter = ComplexProfileConverter(
        click_to_start=True,
        click_to_purge=click_to_purge,
        end_node_head=END_NODE_HEAD,
        init_node_tail=INIT_NODE_TAIL,
        parameters=profile,
    )

    head = converter.head_template()
    manual = manual_stages(profile, converter)
    tail = converter.tail_template()

    program = {
        "name": profile.get("name") or "Profile",
        "id": profile.get("id"),
        "stages": head + manual + tail,
    }
    validate_program(program)
    return program

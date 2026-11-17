//! `manual_program.py`: the node program the ESP32 runs for a manual profile.
//!
//! An ordinary profile describes a curve the machine follows. A manual
//! profile has none: the encoder sets the target live and the shot ends on a
//! long press or on the backend's `finish`. The document only carries the
//! shape of the session (the control the shot starts in, the temperature and
//! the weight that bound it); the nodes built here implement the interaction.
//! The head and tail come from the converter's templates unchanged.
//!
//! Section 8 ("Node program") of the Manual mode cross-repo contract.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::{Map, Value};

use crate::converter::simplified_json::NodeIds;
use crate::converter::triggers::{
    Gesture, Trigger, encoder_button_trigger, piston_position_trigger, predictive_weight_trigger,
    user_finish_trigger,
};
use crate::converter::{
    ComplexProfileConverter, InitNode, MAX_PISTON_POSITION, ReferenceIds, dictionaries,
    nodes::Nodes,
};
use crate::pynum::float_value;
use crate::pyops::{self, PyExc, check_hashable, getitem_index};
use crate::pyrepr::{py_repr, py_strip, truthy};

/// The head's "closing valve" node exits to `END_NODE_HEAD`, the tail's first
/// node is `INIT_NODE_TAIL`; the manual stages sit between them.
pub const END_NODE_HEAD: i64 = 1000;
pub const INIT_NODE_TAIL: i64 = 7000;

/// The ESP's end-of-profile node.
pub const END_STAGE_NODE_ID: i64 = -2;

/// One encoder detent moves the target by `STEP`, clamped to `[MIN, MAX]`.
pub const MANUAL_CONTROLLER_STEP: f64 = 0.1;
pub const MANUAL_CONTROLLER_MIN: f64 = 0.0;
pub const MANUAL_CONTROLLER_MAX: f64 = 12.0;

/// Re-entering a stage starts the target slightly above the sensor reading.
pub const MANUAL_RESUME_SENSOR_GAIN: f64 = 1.1;

/// A manual shot ends when the piston reaches 100 % of its usable travel.
pub const MANUAL_PISTON_LIMIT: i64 = MAX_PISTON_POSITION - 2;

/// The shot-wide weight and position references the head establishes.
pub const SHOT_WEIGHT_REFERENCE_ID: i64 = 1;
pub const SHOT_POSITION_REFERENCE_ID: i64 = 0;

/// The controller kinds that mark a manual stage.
pub const MANUAL_CONTROLLER_KINDS: [&str; 2] =
    ["manual_pressure_controller", "manual_flow_controller"];

/// The config values the program depends on (`MeticulousConfig[CONFIG_USER]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgramSettings {
    /// `auto_purge_after_shot`: purge without waiting for a click.
    pub auto_purge: bool,
    /// `allow_stage_skipping`: the templates add a button exit to most nodes.
    pub allow_stage_skipping: bool,
    /// `partial_retraction` in mm. The Python config always holds a float
    /// here (loading and the settings API both convert it).
    pub partial_retraction: f64,
}

impl Default for ProgramSettings {
    /// The config defaults.
    fn default() -> Self {
        ProgramSettings {
            auto_purge: false,
            allow_stage_skipping: false,
            partial_retraction: 45.33,
        }
    }
}

/// What `build_manual_program` raises.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProgramError {
    /// `ValueError`: the document or the program is not one the ESP can run.
    #[error("{0}")]
    Value(String),
    /// A built-in exception from reading a malformed document
    /// (unreachable for a document that passed schema validation).
    #[error("{}", .0.message())]
    Builtin(PyExc),
}

impl ProgramError {
    pub fn class_name(&self) -> &'static str {
        match self {
            ProgramError::Value(_) => "ValueError",
            ProgramError::Builtin(exc) => exc.class_name(),
        }
    }
}

impl From<PyExc> for ProgramError {
    fn from(exc: PyExc) -> Self {
        ProgramError::Builtin(exc)
    }
}

/// `_MANUAL_CONTROLLERS[stage_type]`.
struct ManualController {
    kind: &'static str,
    algorithm: &'static str,
    sensor: &'static str,
    stage_name: &'static str,
}

fn manual_controller_spec(stage_type: &str) -> Option<ManualController> {
    match stage_type {
        "pressure" => Some(ManualController {
            kind: "manual_pressure_controller",
            algorithm: dictionaries::algorithms_type::PRESSURE_PID_V1,
            sensor: dictionaries::source_type::RAW_PRESSURE,
            stage_name: "Manual pressure",
        }),
        "flow" => Some(ManualController {
            kind: "manual_flow_controller",
            algorithm: dictionaries::algorithms_type::FLOW_PID_V1,
            sensor: dictionaries::source_type::RAW_FLOW,
            stage_name: "Manual flow",
        }),
        _ => None,
    }
}

/// `_manual_controller(stage_type, initial)`.
fn manual_controller(spec: &ManualController, initial: Value) -> Value {
    let mut data = Map::new();
    data.insert("kind".into(), Value::from(spec.kind));
    data.insert("algorithm".into(), Value::from(spec.algorithm));
    data.insert("step".into(), float_value(MANUAL_CONTROLLER_STEP));
    data.insert("min".into(), float_value(MANUAL_CONTROLLER_MIN));
    data.insert("max".into(), float_value(MANUAL_CONTROLLER_MAX));
    data.insert("initial".into(), initial);
    Value::Object(data)
}

/// `_manual_triggers(final_weight, other_resume_node_id)`: the five exits of
/// every manual node, first match wins.
fn manual_triggers(final_weight: &Value, other_resume_node_id: i64) -> [Trigger; 5] {
    [
        encoder_button_trigger(Gesture::Single, other_resume_node_id),
        encoder_button_trigger(Gesture::Long, INIT_NODE_TAIL),
        user_finish_trigger(INIT_NODE_TAIL),
        predictive_weight_trigger(
            final_weight.clone(),
            SHOT_WEIGHT_REFERENCE_ID,
            INIT_NODE_TAIL,
        ),
        piston_position_trigger(
            Value::from(MANUAL_PISTON_LIMIT),
            SHOT_POSITION_REFERENCE_ID,
            INIT_NODE_TAIL,
        ),
    ]
}

/// `_initial_target(stage)`: `float(stage["dynamics"]["points"][0][1])`.
fn initial_target(stage: &Value) -> Result<f64, ProgramError> {
    let dynamics = match stage.get("dynamics") {
        Some(d) if truthy(d) => d,
        _ => &Value::Object(Map::new()),
    };
    if !dynamics.is_object() {
        return Err(pyops::no_attribute_get(dynamics).into());
    }
    let points = dynamics.get("points").unwrap_or(&Value::Null);
    let value = getitem_index(points, 0)
        .and_then(|first| getitem_index(&first, 1))
        .map_err(|_| {
            ProgramError::Value("a manual stage needs a first dynamics point to start from".into())
        })?;
    match &value {
        Value::Number(n) => Ok(crate::pynum::PyNum::from_number(n).as_f64()),
        other => Err(ProgramError::Value(format!(
            "the initial manual target must be a number, got {}",
            py_repr(other)
        ))),
    }
}

/// `_stage_name(stage, stage_type)`: the stage's name unless it is blank.
fn stage_name(stage: &Value, spec: &ManualController) -> Value {
    match stage.get("name") {
        Some(Value::String(name)) if !py_strip(name).is_empty() => Value::String(name.clone()),
        _ => Value::from(spec.stage_name),
    }
}

struct Plan<'a> {
    stage: &'a Value,
    spec: ManualController,
    init_id: i64,
    start_id: Option<i64>,
    resume_id: i64,
}

/// `manual_stages(profile, converter)`: the stages between the head and the
/// tail. Each document stage becomes an `init` node with the stage's own
/// references and a `resume` node driving the manual controller; the first
/// also gets a `start` node, entered once from the head, whose target begins
/// at the document's first point instead of the sensor reading.
pub fn manual_stages(
    profile: &Value,
    converter: &ComplexProfileConverter<'_>,
    reference_ids: &mut ReferenceIds,
) -> Result<Vec<Value>, ProgramError> {
    let stages = match profile.get("stages") {
        Some(Value::Array(stages)) if !stages.is_empty() => stages,
        _ => {
            return Err(ProgramError::Value(
                "a manual profile needs at least one stage".into(),
            ));
        }
    };

    // `simplified_json.current_node_id = END_NODE_HEAD`.
    let mut node_ids = NodeIds::starting_at(END_NODE_HEAD);
    let final_weight = converter.complex.get_final_weight()?;

    let mut plans = Vec::with_capacity(stages.len());
    for (index, stage) in stages.iter().enumerate() {
        if !stage.is_object() {
            return Err(pyops::no_attribute_get(stage).into());
        }
        let stage_type = stage.get("type").unwrap_or(&Value::Null);
        check_hashable(stage_type)?;
        let Some(spec) = stage_type.as_str().and_then(manual_controller_spec) else {
            return Err(ProgramError::Value(format!(
                "unsupported manual stage type {}",
                py_repr(stage_type)
            )));
        };
        let init_id = node_ids.allocate();
        let start_id = (index == 0).then(|| node_ids.allocate());
        let resume_id = node_ids.allocate();
        plans.push(Plan {
            stage,
            spec,
            init_id,
            start_id,
            resume_id,
        });
    }

    let mut program_stages = Vec::with_capacity(plans.len());
    for (index, plan) in plans.iter().enumerate() {
        let entry_id = plan.start_id.unwrap_or(plan.resume_id);

        let mut init_node = InitNode::new(plan.init_id);
        init_node.set_time_id(reference_ids.allocate());
        init_node.set_weight_id(reference_ids.allocate());
        init_node.set_position_id(reference_ids.allocate());
        init_node.set_next_node_id(entry_id);

        // A tap hands the shot to the next stage and wraps around.
        let other_resume_id = plans[(index + 1) % plans.len()].resume_id;
        let mut nodes = vec![init_node.get_node()];

        if let Some(start_id) = plan.start_id {
            let mut start_node = Nodes::new(start_id);
            let mut initial = Map::new();
            initial.insert("kind".into(), Value::from("value"));
            initial.insert("value".into(), float_value(initial_target(plan.stage)?));
            start_node.add_controller(manual_controller(&plan.spec, Value::Object(initial)));
            for trigger in &manual_triggers(final_weight, other_resume_id) {
                start_node.add_trigger(trigger);
            }
            nodes.push(start_node.get_node());
        }

        let mut resume_node = Nodes::new(plan.resume_id);
        let mut initial = Map::new();
        initial.insert("kind".into(), Value::from("sensor"));
        initial.insert("source".into(), Value::from(plan.spec.sensor));
        initial.insert("gain".into(), float_value(MANUAL_RESUME_SENSOR_GAIN));
        resume_node.add_controller(manual_controller(&plan.spec, Value::Object(initial)));
        for trigger in &manual_triggers(final_weight, other_resume_id) {
            resume_node.add_trigger(trigger);
        }
        nodes.push(resume_node.get_node());

        let mut stage = Map::new();
        stage.insert("name".into(), stage_name(plan.stage, &plan.spec));
        stage.insert("nodes".into(), Value::Array(nodes));
        program_stages.push(Value::Object(stage));
    }
    Ok(program_stages)
}

/// `_is_manual_stage(stage)`: a stage with a manual controller.
pub fn is_manual_stage(stage: &Value) -> bool {
    list_or_empty(stage.get("nodes")).iter().any(|node| {
        list_or_empty(node.get("controllers"))
            .iter()
            .any(is_manual_controller)
    })
}

fn is_manual_controller(controller: &Value) -> bool {
    controller
        .get("kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| MANUAL_CONTROLLER_KINDS.contains(&kind))
}

/// `value or []` for a list field.
fn list_or_empty(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

fn node_id(node: &Value) -> Result<i64, ProgramError> {
    match node.get("id") {
        Some(id) => id
            .as_i64()
            .ok_or_else(|| ProgramError::Value(format!("node id {} is not an int", py_repr(id)))),
        None => Err(PyExc::Key("'id'".into()).into()),
    }
}

fn node_ids(group: &[Value]) -> Result<Vec<i64>, ProgramError> {
    let mut ids = Vec::new();
    for stage in group {
        for node in list_or_empty(stage.get("nodes")) {
            ids.push(node_id(node)?);
        }
    }
    Ok(ids)
}

fn int_list(ids: impl IntoIterator<Item = i64>) -> String {
    let items: Vec<String> = ids.into_iter().map(|id| id.to_string()).collect();
    format!("[{}]", items.join(", "))
}

fn target_repr(target: Option<i64>, raw: Option<&Value>) -> String {
    match (target, raw) {
        (Some(id), _) => id.to_string(),
        (None, Some(value)) => py_repr(value),
        (None, None) => "None".into(),
    }
}

/// `validate_program(program)`: fail unless the ESP can run the program.
/// The ESP follows `next_node_id` without checking it, so a dangling id
/// would strand the machine mid-shot with the piston loaded.
pub fn validate_program(program: &Value) -> Result<(), ProgramError> {
    let stages = match program.get("stages") {
        Some(Value::Array(stages)) if !stages.is_empty() => stages,
        _ => return Err(ProgramError::Value("the node program has no stages".into())),
    };

    let manual_indexes: Vec<usize> = stages
        .iter()
        .enumerate()
        .filter(|(_, stage)| is_manual_stage(stage))
        .map(|(index, _)| index)
        .collect();
    let (Some(&first), Some(&last)) = (manual_indexes.first(), manual_indexes.last()) else {
        return Err(ProgramError::Value(
            "the node program has no manual stages".into(),
        ));
    };

    let head = &stages[..first];
    let manual = &stages[first..=last];
    let tail = &stages[last + 1..];

    let manual_ids = node_ids(manual)?;
    let template_ids: Vec<i64> = node_ids(head)?.into_iter().chain(node_ids(tail)?).collect();

    let template_set: HashSet<i64> = template_ids.iter().copied().collect();
    let collisions: BTreeSet<i64> = manual_ids
        .iter()
        .copied()
        .filter(|id| template_set.contains(id))
        .collect();
    if !collisions.is_empty() {
        return Err(ProgramError::Value(format!(
            "manual node ids collide with the head or tail: {}",
            int_list(collisions)
        )));
    }

    let all_ids: Vec<i64> = manual_ids.iter().chain(&template_ids).copied().collect();
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for id in &all_ids {
        *counts.entry(*id).or_default() += 1;
    }
    let duplicates: BTreeSet<i64> = all_ids
        .iter()
        .copied()
        .filter(|id| counts[id] > 1)
        .collect();
    if !duplicates.is_empty() {
        return Err(ProgramError::Value(format!(
            "duplicate node ids in the node program: {}",
            int_list(duplicates)
        )));
    }

    let tail_entry = match tail.first() {
        Some(stage) => match list_or_empty(stage.get("nodes")).first() {
            Some(node) => node_id(node)?,
            None => return Err(ProgramError::Value("the node program has no tail".into())),
        },
        None => return Err(ProgramError::Value("the node program has no tail".into())),
    };
    if tail_entry != INIT_NODE_TAIL {
        return Err(ProgramError::Value(format!(
            "the tail is entered at {tail_entry}, expected {INIT_NODE_TAIL}"
        )));
    }

    let known_ids: HashSet<i64> = all_ids.iter().copied().collect();
    for stage in stages {
        for node in list_or_empty(stage.get("nodes")) {
            for trigger in list_or_empty(node.get("triggers")) {
                let raw = trigger.get("next_node_id");
                let target = raw.and_then(Value::as_i64);
                let known =
                    target.is_some_and(|t| t == END_STAGE_NODE_ID || known_ids.contains(&t));
                if !known {
                    return Err(ProgramError::Value(format!(
                        "node {} of stage {} points at unknown node {}",
                        node_id(node)?,
                        py_repr(stage.get("name").unwrap_or(&Value::Null)),
                        target_repr(target, raw)
                    )));
                }
            }
        }
    }

    let head_ids: HashSet<i64> = node_ids(head)?.into_iter().collect();
    let mut head_exits = BTreeSet::new();
    for stage in head {
        for node in list_or_empty(stage.get("nodes")) {
            // Every target is a known int by now.
            let targets = list_or_empty(node.get("triggers"))
                .iter()
                .filter_map(|trigger| trigger.get("next_node_id").and_then(Value::as_i64));
            for target in targets {
                if !head_ids.contains(&target) && target != END_STAGE_NODE_ID {
                    head_exits.insert(target);
                }
            }
        }
    }
    if head_exits.len() != 1 || !head_exits.contains(&END_NODE_HEAD) {
        return Err(ProgramError::Value(format!(
            "the head exits to {}, expected [{END_NODE_HEAD}]",
            int_list(head_exits)
        )));
    }

    let mut start_ids = Vec::new();
    for node in list_or_empty(manual[0].get("nodes")) {
        for controller in list_or_empty(node.get("controllers")) {
            let starts_at_value = controller
                .get("initial")
                .filter(|initial| truthy(initial))
                .and_then(|initial| initial.get("kind"))
                .and_then(Value::as_str)
                == Some("value");
            if is_manual_controller(controller) && starts_at_value {
                start_ids.push(node_id(node)?);
            }
        }
    }
    let [start_id] = start_ids[..] else {
        return Err(ProgramError::Value(format!(
            "the first manual stage needs exactly one start node, found {}",
            start_ids.len()
        )));
    };

    let mut triggers_by_id: HashMap<i64, Vec<i64>> = HashMap::new();
    for stage in stages {
        for node in list_or_empty(stage.get("nodes")) {
            let targets = list_or_empty(node.get("triggers"))
                .iter()
                .filter_map(|trigger| trigger.get("next_node_id").and_then(Value::as_i64))
                .collect();
            triggers_by_id.insert(node_id(node)?, targets);
        }
    }
    let mut reached = HashSet::new();
    let mut pending = vec![END_NODE_HEAD];
    while let Some(id) = pending.pop() {
        if reached.contains(&id) {
            continue;
        }
        let Some(targets) = triggers_by_id.get(&id) else {
            continue;
        };
        reached.insert(id);
        pending.extend(targets);
    }
    if !reached.contains(&start_id) {
        return Err(ProgramError::Value(format!(
            "the first manual stage's start node {start_id} is unreachable from the head exit {END_NODE_HEAD}"
        )));
    }
    Ok(())
}

/// `build_manual_program(profile)`: the node program for a manual profile
/// document (contract section 8), `{"name", "id", "stages"}`.
pub fn build_manual_program(
    profile: &Value,
    settings: &ProgramSettings,
    reference_ids: &mut ReferenceIds,
) -> Result<Value, ProgramError> {
    let click_to_purge = !settings.auto_purge;
    let converter = ComplexProfileConverter::new(
        click_to_purge,
        END_NODE_HEAD,
        INIT_NODE_TAIL,
        profile,
        settings.allow_stage_skipping,
        settings.partial_retraction,
    )?;

    let head = converter.head_template()?;
    let manual = manual_stages(profile, &converter, reference_ids)?;
    let tail = converter.tail_template();

    let name = match profile.get("name") {
        Some(name) if truthy(name) => name.clone(),
        _ => Value::from("Profile"),
    };
    let mut program = Map::new();
    program.insert("name".into(), name);
    program.insert(
        "id".into(),
        profile.get("id").cloned().unwrap_or(Value::Null),
    );
    program.insert(
        "stages".into(),
        Value::Array(head.into_iter().chain(manual).chain(tail).collect()),
    );
    let program = Value::Object(program);
    validate_program(&program)?;
    Ok(program)
}

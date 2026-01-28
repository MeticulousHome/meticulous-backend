import json
from .stages import *
from .dictionaries_simplified import *
from config import (
    MeticulousConfig,
    CONFIG_USER,
    MACHINE_ALLOW_STAGE_SKIPPING,
    MAX_PISTON_POSITION,
)


current_node_id = 1
current_curve_id = 10000
current_reference_id = 100


class SimplifiedJson:
    """
    A simplified JSON class that allows loading, displaying,
    and converting JSON data to a complex JSON.

    Attributes:
        parameters (dict): A dictionary to store JSON data.
    """

    def __init__(self, parameters: dict = None):
        self.parameters = parameters if parameters is not None else {}

    def load_simplified_json(self, parameters):
        self.parameters = parameters

    def show_simplified_json(self):
        self.parameters = json.dumps(self.parameters, indent=2)
        print(json.dumps(self.parameters, indent=2))

    def get_temperature(self):
        return self.parameters["temperature"]

    def get_name(self):
        return self.parameters["name"]

    def get_final_weight(self):
        return self.parameters["final_weight"]

    def get_new_node_id(self):
        global current_node_id
        current_node_id += 1
        return current_node_id - 1

    def get_new_curve_id(self):
        global current_curve_id
        current_curve_id += 1
        return current_curve_id - 1

    def get_new_reference_id(self):
        global current_reference_id
        current_reference_id += 1
        return current_reference_id - 1

    def set_comparison_type(self, comparison_value=None):
        default_comparison = TriggerOperatorType.GREATER_THAN_OR_EQUAL

        if comparison_value is None:
            comparison_value = default_comparison
            print(
                f"Comparison value is None. Using default value: {default_comparison}."
            )

        if comparison_value == ">=":
            comparison = TriggerOperatorType.GREATER_THAN_OR_EQUAL
        elif comparison_value == "<=":
            comparison = TriggerOperatorType.LESS_THAN_OR_EQUAL
        else:
            comparison = TriggerOperatorType.GREATER_THAN_OR_EQUAL
            print(
                f"Comparison: {comparison_value} not supported. Using default value: >= ."
            )

        return comparison

    def to_complex(self, end_node_head: int, init_node_tail: int):
        global current_node_id
        global current_curve_id
        global current_reference_id

        allow_skipping = MeticulousConfig[CONFIG_USER][MACHINE_ALLOW_STAGE_SKIPPING]

        current_node_id = end_node_head
        # Use the comments with * as debugging tools.
        complex_stages = []
        for stage_index, stage in enumerate(self.parameters.get("stages")):

            init_node = InitNode(self.get_new_node_id())
            main_node = Nodes(self.get_new_node_id())

            init_node.set_next_node_id(main_node.get_node_id())
            init_node.set_time_id(self.get_new_reference_id())
            init_node.set_weight_id(self.get_new_reference_id())
            init_node.set_position_id(self.get_new_reference_id())

            stage_name = stage.get("name")

            exit_triggers = []
            limit_triggers = []
            all_nodes = [init_node.get_node(), main_node.get_node()]

            for limit in stage["limits"]:
                limit_node = None
                limit_trigger = None

                match limit["type"]:
                    case "pressure":
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [[0, trigger_limit_value]]
                        limit_curve_id = self.get_new_curve_id()
                        limit_reference_curve_id = init_node.get_time_id()
                        limit_controller = PressureController(
                            PressureAlgorithmType.PID_V1,
                            limit_curve_id,
                            CurveInterpolationType.LINEAR,
                            points_trigger,
                            ReferenceType.TIME,
                            limit_reference_curve_id,
                        )
                        limit_id = limit_node.get_node_id()
                        limit_trigger = PressureValueTrigger(
                            SourceType.RAW,
                            TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                            trigger_limit_value,
                            limit_id,
                        )
                        limit_node.add_controller(limit_controller)

                    case "flow":
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [[0, trigger_limit_value]]
                        limit_curve_id = self.get_new_curve_id()
                        limit_reference_curve_id = init_node.get_time_id()
                        limit_controller = FlowController(
                            FlowAlgorithmType.PID_V1,
                            limit_curve_id,
                            CurveInterpolationType.LINEAR,
                            points_trigger,
                            ReferenceType.TIME,
                            limit_reference_curve_id,
                        )
                        limit_id = limit_node.get_node_id()
                        limit_trigger = FlowValueTrigger(
                            SourceType.RAW,
                            TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                            trigger_limit_value,
                            limit_id,
                        )
                        limit_node.add_controller(limit_controller)

                    case "temperature":
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [[0, trigger_limit_value]]
                        limit_curve_id = self.get_new_curve_id()
                        limit_reference_curve_id = init_node.get_time_id()
                        limit_controller = TemperatureController(
                            TemperatureAlgorithmType.WATER,
                            limit_curve_id,
                            CurveInterpolationType.LINEAR,
                            points_trigger,
                            ReferenceType.TIME,
                            limit_reference_curve_id,
                        )
                        limit_id = limit_node.get_node_id()
                        limit_trigger = TemperatureValueTrigger(
                            TemperatureSourceType.WATER,
                            TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                            trigger_limit_value,
                            limit_id,
                        )
                        limit_node.add_controller(limit_controller)

                    case "power":
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [[0, trigger_limit_value]]
                        limit_curve_id = self.get_new_curve_id()
                        limit_reference_curve_id = init_node.get_time_id()
                        limit_controller = PowerController(
                            PowerAlgorithmType.SPRING,
                            limit_curve_id,
                            CurveInterpolationType.LINEAR,
                            points_trigger,
                            ReferenceType.TIME,
                            limit_reference_curve_id,
                        )
                        limit_id = limit_node.get_node_id()
                        limit_trigger = PowerValueTrigger(
                            SourceType.RAW,
                            TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                            trigger_limit_value,
                            limit_id,
                        )
                        limit_node.add_controller(limit_controller)

                    case _:
                        print(f"Limit type: {limit['type']} not found.")
                all_nodes.append(limit_node.get_node())
                limit_triggers.append(limit_trigger.get_trigger())

            next_stage_node_id = self.get_new_node_id()
            current_node_id = next_stage_node_id

            for exits in stage["exit_triggers"]:
                json_comparison = exits.get("comparison")
                match exits["type"]:
                    case "time":
                        exit_trigger_value = exits["value"]
                        if exits["relative"]:
                            reference_id = init_node.get_time_id()
                        else:
                            reference_id = 1
                        time_comparison = self.set_comparison_type(json_comparison)
                        exit_trigger = TimerTrigger(
                            time_comparison,
                            exit_trigger_value,
                            reference_id,
                            next_stage_node_id,
                        )
                        exit_triggers.append(exit_trigger.get_trigger())
                        # print(F"Next Stage Node ID after match: {next_stage_node_id} from the stage {stage_name}")

                    case "weight":
                        exit_trigger_value = exits["value"]
                        if exits["relative"]:
                            reference_id = init_node.get_weight_id()
                        else:
                            reference_id = 1
                        weight_comparison = self.set_comparison_type(json_comparison)
                        exit_trigger = WeightTrigger(
                            SourceType.PREDICTIVE,
                            weight_comparison,
                            exit_trigger_value,
                            reference_id,
                            next_stage_node_id,
                        )
                        exit_triggers.append(exit_trigger.get_trigger())

                    case "pressure":
                        pressure_comparison = self.set_comparison_type(json_comparison)
                        exit_trigger_value = exits["value"]
                        exit_trigger = PressureValueTrigger(
                            SourceType.RAW,
                            pressure_comparison,
                            exit_trigger_value,
                            next_stage_node_id,
                        )
                        exit_triggers.append(exit_trigger.get_trigger())

                    case "flow":
                        flow_comparison = self.set_comparison_type(json_comparison)
                        exit_trigger_value = exits["value"]
                        exit_trigger = FlowValueTrigger(
                            SourceType.RAW,
                            flow_comparison,
                            exit_trigger_value,
                            next_stage_node_id,
                        )
                        exit_triggers.append(exit_trigger.get_trigger())

                    case "piston_position":
                        # Convert percentage (0-100) to position in mm
                        exit_trigger_value_percent = exits["value"]
                        exit_trigger_value = (exit_trigger_value_percent / 100) * (
                            MAX_PISTON_POSITION - 2
                        )
                        if exits["relative"]:
                            reference_id = init_node.get_position_id()
                        else:
                            reference_id = 0
                        piston_position_comparison = self.set_comparison_type(
                            json_comparison
                        )
                        exit_trigger = PistonPositionTrigger(
                            piston_position_comparison,
                            exit_trigger_value,
                            reference_id,
                            next_stage_node_id,
                        )
                        exit_triggers.append(exit_trigger.get_trigger())

                    case "power":
                        power_comparison = self.set_comparison_type(json_comparison)
                        exit_trigger_value = exits["value"]
                        exit_trigger = PowerValueTrigger(
                            SourceType.RAW,
                            power_comparison,
                            exit_trigger_value,
                            next_stage_node_id,
                        )
                        exit_triggers.append(exit_trigger.get_trigger())

                    case "temperature":
                        temperature_comparison = self.set_comparison_type(
                            json_comparison
                        )
                        exit_trigger_value = exits["value"]
                        exit_trigger = TemperatureValueTrigger(
                            TemperatureSourceType.WATER,
                            temperature_comparison,
                            exit_trigger_value,
                            next_stage_node_id,
                        )
                        exit_triggers.append(exit_trigger.get_trigger())
                    case _:
                        print(f"Exit type: {exits['type']} not found.")
            for limit_node in all_nodes[2:]:
                limit_node["triggers"] += [
                    trigger
                    for trigger in limit_triggers
                    if trigger["next_node_id"] != limit_node["id"]
                ]
                limit_node["triggers"] += exit_triggers

                # limit_node_id = limit_node["id"] # *Get the limit node id from the limit node of the stages
                # trigger_next_node_id = limit_node["triggers"][0]["next_node_id"] # *Get the next node id from the exit triggers of the stages

            for trigger in exit_triggers:
                trigger = Triggers(trigger)
                main_node.add_trigger(trigger)
            for trigger in limit_triggers:
                trigger = Triggers(trigger)
                main_node.add_trigger(trigger)

            dynamics = stage.get("dynamics")
            type_main_controller = stage.get("type")
            points_main_controller = dynamics.get("points")
            over_main_controller = dynamics.get("over")
            interpolation_main_controller = dynamics.get("interpolation")
            main_node_id = main_node.get_node_id()

            match over_main_controller:
                case "time":
                    main_reference_curve_id = init_node.get_time_id()
                case "weight":
                    main_reference_curve_id = init_node.get_weight_id()
                case "piston_position":
                    main_reference_curve_id = init_node.get_position_id()

            match type_main_controller:
                case "pressure":
                    main_curve_id_generate = self.get_new_curve_id()
                    main_controller = PressureController(
                        PressureAlgorithmType.PID_V1,
                        main_curve_id_generate,
                        interpolation_dict[interpolation_main_controller],
                        points_main_controller,
                        over_dict[over_main_controller],
                        main_reference_curve_id,
                    )
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = PressureCurveTrigger(
                        SourceType.RAW,
                        TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                        main_curve_id,
                        main_node_id,
                    )
                    main_trigger = main_trigger.get_trigger()

                case "flow":
                    main_curve_id_generate = self.get_new_curve_id()
                    main_controller = FlowController(
                        FlowAlgorithmType.PID_V1,
                        main_curve_id_generate,
                        interpolation_dict[interpolation_main_controller],
                        points_main_controller,
                        over_dict[over_main_controller],
                        main_reference_curve_id,
                    )
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = FlowCurveTrigger(
                        SourceType.RAW,
                        TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                        main_curve_id,
                        main_node_id,
                    )
                    main_trigger = main_trigger.get_trigger()

                case "temperature":
                    main_curve_id_generate = self.get_new_curve_id()
                    main_controller = TemperatureController(
                        TemperatureAlgorithmType.WATER,
                        main_curve_id_generate,
                        interpolation_dict[interpolation_main_controller],
                        points_main_controller,
                        over_dict[over_main_controller],
                        main_reference_curve_id,
                    )
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = TemperatureCurveTrigger(
                        TemperatureSourceType.WATER,
                        TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                        main_curve_id,
                        main_node_id,
                    )
                    main_trigger = main_trigger.get_trigger()

                case "power":
                    main_curve_id_generate = self.get_new_curve_id()
                    main_controller = PowerController(
                        PowerAlgorithmType.SPRING,
                        main_curve_id_generate,
                        interpolation_dict[interpolation_main_controller],
                        points_main_controller,
                        over_dict[over_main_controller],
                        main_reference_curve_id,
                    )
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = PowerCurveTrigger(
                        SourceType.RAW,
                        TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                        main_curve_id,
                        main_node_id,
                    )
                    main_trigger = main_trigger.get_trigger()

                case _:
                    print(f"Type: {type_main_controller} not found.")

            button_trigger = ButtonTrigger(
                ButtonSourceType.ENCODER_BUTTON, next_node_id=next_stage_node_id
            )
            weight_final_trigger = WeightTrigger(
                SourceType.PREDICTIVE,
                TriggerOperatorType.GREATER_THAN_OR_EQUAL,
                self.get_final_weight(),
                1,
                init_node_tail,
            )

            for limit_node in all_nodes[2:]:
                limit_node["triggers"].append(main_trigger)
                if allow_skipping:
                    limit_node["triggers"].append(button_trigger.get_trigger())
                limit_node["triggers"].append(weight_final_trigger.get_trigger())

            if allow_skipping:
                main_node.add_trigger(button_trigger)
            main_node.add_trigger(weight_final_trigger)

            if stage_index == len(self.parameters.get("stages")) - 1:
                if allow_skipping:
                    button_trigger.set_next_node_id(init_node_tail)
                weight_final_trigger.set_next_node_id(init_node_tail)
                for exit_trigger in exit_triggers:
                    exit_trigger["next_node_id"] = init_node_tail

            complex_stages.append({"name": f"{stage_name}", "nodes": all_nodes})

        # print(f"Complex stage nodes from the {stage_name}:") # *Print the complex stages with the stage's name
        # print(json.dumps(complex_stages, indent=2)) # *Print the complex stages with json format to see the changes.
        return complex_stages


class InitNode(Nodes):
    def __init__(
        self,
        id: int = -1,
        time_ref_id: int = 1,
        weight_ref_id: int = 2,
        position_ref_id: int = 3,
        next_node_id: int = -1,
    ):
        super().__init__()
        self.set_id(id)
        if time_ref_id is not None:
            self.time_reference = TimeReferenceController()
            self.set_time_id(time_ref_id)
        if weight_ref_id is not None:
            self.weight_reference = WeightReferenceController()
            self.set_weight_id(weight_ref_id)
        if position_ref_id is not None:
            self.position_reference = PositionReferenceController()
            self.set_position_id(position_ref_id)
        self.exit_trigger = ExitTrigger(next_node_id)
        self.add_controller(self.time_reference)
        self.add_controller(self.weight_reference)
        self.add_controller(self.position_reference)
        self.add_trigger(self.exit_trigger)

    def set_time_id(self, id: int):
        self.time_reference.set_reference_id(id)

    def set_weight_id(self, id: int):
        self.weight_reference.set_reference_id(id)

    def set_position_id(self, id: int):
        self.position_reference.set_reference_id(id)

    def get_time_id(self):
        return self.time_reference.get_time_reference_id()

    def get_weight_id(self):
        return self.weight_reference.get_weight_id()

    def get_position_id(self):
        return self.position_reference.get_position_reference_id()

    def set_next_node_id(self, id: int):
        self.exit_trigger.set_next_node_id(id)


if __name__ == "__main__":
    # Example usage of the SimplifiedJson class.

    file_path = "simplified_json_example.json"
    with open(file_path, "r") as file:
        data = json.load(file)

    simplified_json = SimplifiedJson(data)
    simplified_json.load_simplified_json(data)  # Another way to load the JSON data.
    # print(simplified_json.show_simplified_json())
    # print(simplified_json.get_temperature())
    # print(json.dumps(simplified_json.to_complex(), indent=2))
    # print(simplified_json.main_node(1))

    complex_node = simplified_json.to_complex(1000, 5000)
    print(json.dumps(complex_node, indent=2))

    points = [[0, 6], [10, 8]]
    trigger = WeightTrigger(
        SourceType.AVERAGE, TriggerOperatorType.GREATER_THAN, 10, 12
    )

    # print(f"Node ID: {main_node.get_node_id()}")
    # print(json.dumps(main_node.get_node(), indent=2))

    # print(json.dumps(main_node.get_node(), indent=2)) # Uncomment to see the example.

    # Example of initializing a node with references to time, weight, and position.
    init_node = InitNode(-1)
    # print(json.dumps(init_node.get_init_node(), indent=2)) # Uncomment to see the example.

    # Example of setting the time, weight, and position reference IDs.
    init_node_1 = InitNode(-100)
    init_node_1.set_time_id(15)
    init_node_1.set_weight_id(16)
    init_node_1.set_position_id(17)
    init_node_1.set_next_node_id(18)
    # print(init_node_1["triggers"])

    # print(json.dumps(init_node_1.get_node(), indent=2)) # Uncomment to see the example.

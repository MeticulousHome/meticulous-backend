import json

def get_preinfusion_stage(parameters: dict, start_node: int, end_node: int):
    stages_list = []
    values = parameters["stages"]
    
    for stage in values:
        name = stage["name"]
        points_controller = stage["parameters"]["points"]
        stop_weight = stage["parameters"]["stop_weight"]
        max_time = stage["parameters"]["stop_time"]
        max_limit_trigger = stage["parameters"]["max_limit_trigger"]
        interpolation_kind = stage["parameters"]["interpolation_method"]
        if "parameters" in stage and "control_method" in stage["parameters"]:
            if stage["parameters"]["control_method"] == "flow":
                control_kind = "pressure_controller"
                algorithm = "Pressure PID v1.0"
                press_flow_triger = "flow_value_trigger"
                trigger_source = "Flow Raw"
                control_kind_secondary = "flow_controller"
                algorithm_secondary = "Flow PID v1.0"
                pressure_flow_curve_trigger = "pressure_flow_curve_trigger"
                curve_trigger_source = "Pressure Raw"
                
            if stage["parameters"]["control_method"] == "pressure":
                control_kind = "flow_controller"
                algorithm = "Flow PID v1.0"
                press_flow_triger = "pressure_value_trigger"
                trigger_source = "Pressure Raw"
                control_kind_secondary = "pressure_controller"
                algorithm_secondary = "Pressure PID v1.0"
                pressure_flow_curve_trigger = "pressure_flow_curve_trigger"
                curve_trigger_source = "Flow Raw"

        curve_template ={
            "name": name,
            "nodes": [
                {
                    "id": start_node,
                    "controllers": [
                        {
                            "kind": "time_reference",
                            "id": 4
                        }
                    ],
                    "triggers": [
                        {
                            "kind": "exit",
                            "next_node_id": 13
                        }
                    ]
                },
                {
                    "id": 13,
                    "controllers": [
                        {
                            "kind": "temperature_controller",
                            "algorithm": "Cylinder Temperature PID v1.0",
                            "curve": {
                                "id": 12,
                                "interpolation_kind": "linear_interpolation",
                                "points": [
                                    [0, 25.0]
                                ],
                                "time_reference_id": 4
                            }
                        },
                        {
                            "kind": control_kind,
                            "algorithm": algorithm,
                            "curve": {
                                "id": 7,
                                "interpolation_kind": interpolation_kind,
                                "points": points_controller,
                                "time_reference_id": 4
                            }
                        },
                        {
                            "kind": "position_reference",
                            "id": 1
                        }
                    ],
                    "triggers": [
                        {
                            "kind": "timer_trigger",
                            "timer_reference_id": 4,
                            "operator": ">=",
                            "value": max_time,
                            "next_node_id": end_node
                        },
                        {
                            "kind": "weight_value_trigger",
                            "source": "Weight Raw",
                            "weight_reference_id": 1,
                            "operator": ">=",
                            "value": stop_weight,
                            "next_node_id": end_node
                        },
                        {
                            "kind": press_flow_triger,
                            "source": trigger_source,
                            "operator": ">=",
                            "value": max_limit_trigger,
                            "next_node_id": 20
                        },
                        {
                            "kind": "button_trigger",
                            "source": "Encoder Button",
                            "gesture": "Single Tap",
                            "next_node_id": end_node
                        }
                    ]
                },
                {
                    "id": 20,
                    "controllers": [
                        {
                            "kind": control_kind_secondary,
                            "algorithm": algorithm_secondary,
                            "curve": {
                                "id": 9,
                                "interpolation_kind": "catmull_interpolation",
                                "points": [
                                    [0, max_limit_trigger]
                                ],
                                "time_reference_id": 4
                            }
                        },
                        {
                            "kind": "position_reference",
                            "id": 1
                        }
                    ],
                    "triggers": [
                        {
                            "kind": "timer_trigger",
                            "timer_reference_id": 4,
                            "operator": ">=",
                            "value": max_time,
                            "next_node_id": end_node
                        },
                        {
                            "kind": "weight_value_trigger",
                            "source": "Weight Raw",
                            "weight_reference_id": 1,
                            "operator": ">=",
                            "value": stop_weight,
                            "next_node_id": end_node
                        },
                        {
                            "kind": pressure_flow_curve_trigger,
                            "source": curve_trigger_source,
                            "operator": ">=",
                            "curve_id": 7,
                            "next_node_id": end_node
                        },
                        {
                            "kind": "button_trigger",
                            "source": "Encoder Button",
                            "gesture": "Single Tap",
                            "next_node_id": end_node
                        }
                    ]
                }
            ]
        }
        #stages_list.append(curve_template)
    return curve_template


if __name__ == '__main__':
    # Read the JSON file
    with open("../dashboard.json", "r") as f:
        payload = json.load(f)

    stage_modified = get_preinfusion_stage(payload, 300, 301)
    print(stage_modified[1])
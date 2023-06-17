import json

#       "name": "stage 1",
#       "kind": "curve_1.0",
#       "parameters": {
#         "points": [[0,4],[30,4]] -> points_controller,
#         "stop_weight": 1.0, -> stop_weight
#         "stop_time": 35.0, -> max_time
#         "max_limit_trigger": 8.0,
#         "control_method": "flow",
#         "interpolation_method": "linear" -> interpolation_kind
#         "algorithm_main": "Pressure PID v1.0"
#         "control_kind_main": "pressure_controller" -> "kind": "pressure_controller"
#          press_flow_triger -> "kind": "flow_value_trigger"
#          trigger_source -> "source": "Pressure Raw"
#          value_press_flow_triger -> "value": 8.0
#          control_kind_secondary": "pressure_controller" -> "kind": "flow_controller"
#         algorithm_secondary ->"algorithm": "Flow PID v1.0",
#         pressure_flow_curve_trigger -> "kind": "pressure_flow_curve_trigger"
#         curve_trigger_source -> "source": "Pressure Raw"

def get_curve_stage(parameters: json, start_node: int, end_node: int):

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
                        "value": value_press_flow_triger,
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

def get_modified_stages(payload):
    current_node = 300
    stages = payload["stages"]
    json_stages = []
    for stage in stages:
        json_stages.append(stage)
        current_node = current_node + 1

    return json_stages

if __name__ == '__main__':
    # Read the JSON file
    with open("dashboard.json", "r") as f:
        payload = json.load(f)

    # get_curve_stage(payload, 300, 301)
    print(get_modified_stages(payload))
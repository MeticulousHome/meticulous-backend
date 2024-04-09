import json

def get_end_purge_stage(parameters: json, start_node: int, end_node: int):
    max_piston_position = 82
    end_purge_stage =      {
      "name": "purge",
      "nodes": [
        {   
            "id": start_node,
            "controllers": [
             {
                "kind": "move_piston_controller",
                "algorithm": "Piston Ease-In",
                "direction": "DOWN",
                "speed": 6
             },
             {
                "kind": "time_reference",
                "id": 12
             }
            ],
            "triggers": [
            {
                "kind": "exit",
                "next_node_id": 17
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
            "id": 17,
            "controllers": [],
            "triggers": [
                {
                    "kind": "pressure_value_trigger",
                    "next_node_id": 18,
                    "source": "Pressure Raw",
                    "operator": ">=",
                    "value": 6
                },
                {
                    "kind": "timer_trigger",
                    "timer_reference_id": 12,
                    "operator": ">=",
                    "value": 1.5,
                    "next_node_id": 28
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
            "id": 18,
            "controllers": [
                {
                    "kind": "pressure_controller",
                    "algorithm": "Pressure PID v1.0",
                    "curve": {
                        "id": 1,
                        "interpolation_kind": "linear_interpolation",
                        "points": [
                        [
                            0,
                            6
                        ]
                        ],
                        "reference": {
                        "kind": "time",
                        "id": 8
                        }
                    }
                }
            ],
            "triggers": [
                {
                    "kind": "piston_speed_trigger",
                    "operator": "==",
                    "value": 0,
                    "next_node_id": 29
                },
                {
                    "kind": "button_trigger",
                    "next_node_id": end_node,
                    "gesture": "Single Tap",
                    "source": "Encoder Button"
                }
            ]
        },
        {
            "id": 28,
            "controllers": [
                {
                    "kind": "move_piston_controller",
                    "algorithm": "Piston Ease-In",
                    "direction": "DOWN",
                    "speed": 6
                }
            ],
            "triggers": [
                {
                    "kind": "piston_speed_trigger",
                    "operator": "==",
                    "value": 0,
                    "next_node_id": 29
                },
                {
                    "kind": "pressure_value_trigger",
                    "source": "Pressure Raw",
                    "operator": ">=",
                    "value": 6,
                    "next_node_id": 18
                },
                {
                    "kind": "button_trigger",
                    "next_node_id": end_node,
                    "gesture": "Single Tap",
                    "source": "Encoder Button"
                }
                ]
        },
        {
            "id": 29,
            "controllers": [
                {
                    "kind" : "time_reference",
                    "id": 7
                }
                ],
                    "triggers": [
                {
                    "kind": "exit",
                    "next_node_id": 31
                }
                ]
        },
        {
            "id": 31,
            "controllers": [],
            "triggers": [
                {
                    "kind": "piston_speed_trigger",
                    "operator": "!=",
                    "value": 0,
                    "next_node_id": 29
                },
                {
                    "kind": "timer_trigger",
                    "timer_reference_id": 7,
                    "operator": ">=",
                    "value": 1,
                    "next_node_id": 33
                },
                {
                    "kind": "button_trigger",
                    "next_node_id": end_node,
                    "gesture": "Single Tap",
                    "source": "Encoder Button"
                }
            ]
        },
        {
            "id": 33,
            "controllers": [],
            "triggers" : [
                {
                    "kind": "timer_trigger",
                    "timer_reference_id": 8,
                    "operator": ">=",
                    "value": 1,
                    "next_node_id": end_node
                },
                {
                    "kind": "button_trigger",
                    "next_node_id": end_node,
                    "gesture": "Single Tap",
                    "source": "Encoder Button"
                }
            ]
        }
        ]
    }
    
    return end_purge_stage
    # return {}


if __name__ == '__main__':
    
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    end_purge_stage = get_end_purge_stage(json_parameters,0, 1)
    print(json.dumps(end_purge_stage, indent=4))
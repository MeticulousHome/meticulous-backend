import json

def get_prepurge_stage(parameters: json,start_node: int, end_node: int):
    
    max_piston_position = 82
    prepurge_stage = {
        "name": "purge",
        "nodes": [
            {
                "id": start_node,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 29
                    },
                    {
                        "kind": "move_piston_controller",
                        "algorithm": "Piston Fast",
                        "direction": "DOWN",
                        "speed": 6
                    }
                ],
                "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": 2
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
                "id": 2,
                "controllers": [],
                "triggers": [
                    {
                        "kind": "pressure_value_trigger",
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 6.0,
                        "next_node_id": 3
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 29,
                        "operator": ">=",
                        "value": 1.5,
                        "next_node_id": 21
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
                "id": 3,
                "controllers": [
                    {
                        "kind": "pressure_controller",
                        "algorithm": "Pressure PID v1.0",
                        "curve": {
                            "id": 13,
                            "interpolation_kind": "linear_interpolation",
                            "points": [
                                [
                                    0,
                                    6
                                ]
                            ],
                            "time_reference_id": 29
                        }
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_speed_trigger",
                        "operator": "==",
                        "value": 0,
                        "next_node_id": 22
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
            "id": 21,
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
                        "next_node_id": 22
                    },
                    {
                        "kind": "pressure_value_trigger",
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 6,
                        "next_node_id": 3
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
            "id": 22,
            "controllers": [
                    {
                        "kind" : "time_reference",
                        "id": 21
                    }
                ],
                        "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": 23
                    }
                ]
            },
            {
            "id": 23,
            "controllers": [],
            "triggers": [
                    {
                        "kind": "piston_speed_trigger",
                        "operator": "!=",
                        "value": 0,
                        "next_node_id": 22
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 21,
                        "operator": ">=",
                        "value": 1,
                        "next_node_id": 24
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
            "id": 24,
            "controllers": [],
            "triggers" : [
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 29,
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

    # The merged dictionary is now in pre_purge_stage
    return prepurge_stage
    # return {}

if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    prepurge_stage = get_prepurge_stage(json_parameters, 200, 1)
    print(json.dumps(prepurge_stage, indent=4))
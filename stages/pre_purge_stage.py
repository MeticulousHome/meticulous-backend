import json

def get_pre_purge_stage(parameters: json,start_node: int, end_node: int):
    
    pre_purge_stage = {
        "name": "purge",
        "nodes": [
            {
                "id": start_node,
                "controllers": [],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "operator": ">=",
                        "value": 60,
                        "next_node_id": 5,
                        "source": "Piston Position Raw"
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "operator": "<",
                        "value": 60,
                        "next_node_id": 2,
                        "source": "Piston Position Raw"
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": 5
                    }
                ]
            },
            {
                "id": 2,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "algorithm": "Piston Ease-In",
                        "direction": "DOWN",
                        "speed": 6.0
                    },
                    {
                        "kind": "time_reference",
                        "id": 30
                    }
                ],
                "triggers": [
                    {
                        "kind": "pressure_value_trigger",
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 6.0,
                        "next_node_id": 3
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 60,
                        "next_node_id": 5
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": 5
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
                            "time_reference_id": 30
                        }
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 60,
                        "next_node_id": 5
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": 5
                    }
                ]
            },
            {
                "id": 5,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 2
                    },
                    {
                        "kind": "move_piston_controller",
                        "algorithm": "Piston Ease-In",
                        "direction": "DOWN",
                        "speed": 0.0
                    }
                ],
                "triggers": [
                    {
                        "kind": "water_detection_trigger",
                        "value": true,
                        "next_node_id": end_node
                    },
                    {
                        "kind": "water_detection_trigger",
                        "value": false,
                        "next_node_id": 6
                    }
                ]
            },
            {
                "id": 6,
                "controllers": [
                    {
                        "kind": "log_controller",
                        "message": "No Water"
                    }
                ],
                "triggers": [
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 2,
                        "operator": ">=",
                        "value": 2,
                        "next_node_id": 5
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 1,
                        "operator": ">=",
                        "value": 100,
                        "next_node_id": -2
                    }
                ]
            }
        ]
    }
    
    return pre_purge_stage




if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    pre_purge_stage = get_pre_purge_stage(json_parameters, 200, 1)
    print(json.dumps(pre_purge_stage, indent=4))
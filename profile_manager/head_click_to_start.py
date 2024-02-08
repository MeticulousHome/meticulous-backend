import json

def get_head_click_to_start(parameters: json,start_node: int, end_node: int):
    
    head_click_to_start = {
[
    {
        "name": "purge",
        "nodes": [
            {
                "id": -1,
                "controllers": [],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": 45,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 78
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": 6,
                        "source": "Piston Position Raw",
                        "operator": "<",
                        "value": 78
                    }
                ]
            },
            {
                "id": 6,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "speed": 6,
                        "direction": "DOWN",
                        "algorithm": "Piston Ease-In"
                    },
                    {
                        "kind": "time_reference",
                        "id": 3
                    }
                ],
                "triggers": [
                    {
                        "kind": "pressure_value_trigger",
                        "next_node_id": 11,
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 6
                    },
                    {
                        "kind": "piston_position_trigger",  
                        
                        "position_reference_id": 0,
                        "next_node_id": 45,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 78
                    }
                ]
            },
            {
                "id": 11,
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
                                "id": 3
                            }
                        }
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": 45,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 78
                    }
                ]
            }
        ]
    },
    {
        "name": "water detection",
        "nodes": [
            {
                "id": 12,
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
                        "next_node_id": 9,
                        "operator": ">=",
                        "value": 2
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 12,
                        "next_node_id": -2,
                        "operator": ">=",
                        "value": 100
                    }
                ]
            },
            {
                "id": 45,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 12
                    }
                ],
                "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": 9
                    }
                ]
            },
            {
                "id": 9,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 2
                    }
                ],
                "triggers": [
                    {
                        "kind": "water_detection_trigger",
                        "next_node_id": 15,
                        "value": true
                    },
                    {
                        "kind": "water_detection_trigger",
                        "next_node_id": 12,
                        "value": false
                    }
                ]
            }
        ]
    },
    {
        "name": "heating",
        "nodes": [
            {
                "id": 15,
                "controllers": [
                    {
                        "kind": "temperature_controller",
                        "algorithm": "Water Temperature PID v1.0",
                        "curve": {
                            "id": 2,
                            "interpolation_kind": "linear_interpolation",
                            "points": [
                                [
                                    0,
                                    TARGET_TEMPERATURE
                                ]
                            ],
                            "reference": {
                                "kind": "time",
                                "id": 2
                            }
                        }
                    },
                    {
                        "kind": "position_reference",
                        "id": 1
                    }
                ],
                "triggers": [
                    {
                        "kind": "temperature_value_trigger",
                        "next_node_id": 16,
                        "source": "Water Temperature",
                        "operator": ">=",
                        "value": TARGET_TEMPERATURE
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 2,
                        "next_node_id": 14,
                        "operator": ">=",
                        "value": 900
                    }
                ]
            }
        ]
    },
    {
        "name": "click to start",
        "nodes": [
            {
                "id": 16,
                "controllers": [
                    {
                        "kind": "log_controller",
                        "message": "Click to start"
                    }
                ],
                "triggers": [
                    {
                        "kind": "button_trigger",
                        "next_node_id": 17,
                        "source": "Encoder Button"
                    }
                ]
            }
        ]
    },
    {
        "name": "retracting",
        "nodes": [
            {
                "id": 18,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "speed": 6,
                        "direction": "UP",
                        "algorithm": "Piston Ease-In"
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_speed_trigger",
                        "next_node_id": 21,
                        "operator": "==",
                        "value": 0
                    }
                ]
            },
            {
                "id": 21,
                "controllers": [
                    {
                        "kind": "tare_controller"
                    },
                    {
                        "kind": "time_reference",
                        "id": 4
                    }
                ],
                "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": 22
                    }
                ]
            },
            {
                "id": 22,
                "controllers": [
                    {
                        "kind": "weight_reference",
                        "id": 1
                    }
                ],
                "triggers": [
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 4,
                        "next_node_id": 23,
                        "operator": ">=",
                        "value": 2
                    }
                ]
            },
            {
                "id": 17,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "speed": 4,
                        "direction": "UP",
                        "algorithm": "Piston Ease-In"
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "next_node_id": 18,
                        "source": "Piston Position Raw",
                        "position_reference_id": 1,
                        "operator": "<=",
                        "value": -2
                    }
                ]
            }
        ]
    },
    {
        "name": "closing valve",
        "nodes": [
            {
                "id": 23,
                "controllers": [
                    {
                        "kind": "temperature_controller",
                        "algorithm": "Cylinder Temperature PID v1.0",
                        "curve": {
                            "id": 6,
                            "interpolation_kind": "linear_interpolation",
                            "points": [
                                [
                                    0,
                                    25
                                ]
                            ],
                            "reference": {
                                "kind": "time",
                                "id": 9
                            }
                        }
                    },
                    {
                        "kind": "move_piston_controller",
                        "speed": 5,
                        "direction": "DOWN",
                        "algorithm": "Piston Ease-In"
                    },
                    {
                        "kind": "time_reference",
                        "id": 1
                    }
                ],
                "triggers": [
                    {
                        "kind": "pressure_value_trigger",
                        "next_node_id": 34,
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 0.2
                    }
                ]
            }
        ]
    }
]
    }
    
    return head_click_to_start
    # return {}


# if __name__ == '__main__':
  
#     parameters = '{"preheat": true,"temperature": 200}'

#     json_parameters = json.loads(parameters)

#     closing_valve_stage = get_head_click_to_start(json_parameters, 200, 1)
#     print(json.dumps(closing_valve_stage, indent=4))
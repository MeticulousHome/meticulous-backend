import json

def get_tail_click_to_purge(parameters: json,start_node: int, end_node: int):
    
    tail_click_to_purge = {
[
    {
    "name": "retracting",
    "nodes": [
        {
            "id": 25,
            "controllers": [
                {
                "kind": "position_reference",
                "id": 3
                }
            ],
            "triggers": [
                {
                "kind": "exit",
                "next_node_id": 24
                }
            ]
        },
        {
            "id": 27,
            "controllers": [
                {
                "kind": "move_piston_controller",
                "speed": 6,
                "direction": "UP",
                "algorithm": "Piston Fast"
                }
            ],
            "triggers": [
                {
                "kind": "piston_speed_trigger",
                "next_node_id": 30,
                "operator": "==",
                "value": 0
                }
            ]
        },
        {
            "id": 24,
            "controllers": [
                {
                "kind": "move_piston_controller",
                "speed": 4,
                "direction": "UP",
                "algorithm": "Piston Fast"
                }
            ],
            "triggers": [
                {
                "kind": "piston_position_trigger",
                "next_node_id": 27,
                "source": "Piston Position Raw",
                "position_reference_id": 3,
                "operator": "<=",
                "value": -4
                }
            ]
        }
    ]
    },
    {
    "name": "click to purge",
    "nodes": [
        {
            "id": 30,
            "controllers": [
                {
                "kind": "log_controller",
                "message": "Click to purge"
                }
            ],
            "triggers": [
                {
                "kind": "button_trigger",
                "next_node_id": 31,
                "source": "Encoder Button"
                }
            ]
        }
    ]
    },
    {
    "name": "purge",
    "nodes": [
        {
            "id": 31,
            "controllers": [
                {
                "kind": "move_piston_controller",
                "speed": 6,
                "direction": "DOWN",
                "algorithm": "Piston Ease-In"
                },
                {
                "kind": "time_reference",
                "id": 8
                }
            ],
            "triggers": [
                {
                "kind": "pressure_value_trigger",
                "next_node_id": 32,
                "source": "Pressure Raw",
                "operator": ">=",
                "value": 6
                },
                {
                "kind": "piston_position_trigger",
                "position_reference_id": 0,
                "next_node_id": -2,
                "source": "Piston Position Raw",
                "operator": ">=",
                "value": 78
                }
            ]
        },
        {
            "id": 32,
            "controllers": [
                {
                "kind": "pressure_controller",
                "algorithm": "Pressure PID v1.0",
                "curve": {
                    "id": 5,
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
                "kind": "piston_position_trigger",
                "position_reference_id": 0,
                "next_node_id": -2,
                "source": "Piston Position Raw",
                "operator": ">=",
                "value": 78
                }
            ]
        }
    ]
    },
    {
    "name": "END_STAGE",
    "nodes": [
        {
            "id": -2,
            "controllers": [
                {
                "kind": "end_profile"
                }
            ],
            "triggers": []
        }
    ]
    }
]
    }
    
    return tail_click_to_purge
    # return {}


# if __name__ == '__main__':
  
#     parameters = '{"preheat": true,"temperature": 200}'

#     json_parameters = json.loads(parameters)

#     closing_valve_stage = get_head_click_to_start(json_parameters, 200, 1)
#     print(json.dumps(closing_valve_stage, indent=4))
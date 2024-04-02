import json

def get_prepurge_stage(parameters: json,start_node: int, end_node: int):
    
    max_piston_position = 82
    prepurge_stage = {
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
                        "value": max_piston_position,
                        "next_node_id": end_node,
                        "source": "Piston Position Raw"
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "operator": "<",
                        "value": max_piston_position,
                        "next_node_id": 2,
                        "source": "Piston Position Raw"
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
                        "value": max_piston_position,
                        "next_node_id": end_node
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
                        "value": max_piston_position,
                        "next_node_id": end_node
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": end_node
                    }
                ]
            },
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
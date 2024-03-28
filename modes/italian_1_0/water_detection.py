import json

def get_water_detection_stage(parameters: json,start_node: int, end_node: int):
    
    water_detection_stage = {
        "name": "water_detection",
        "nodes": [
            {
                "id": start_node,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 13
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
                        "kind": "exit",
                        "next_node_id": 16
                    }
                ]
            },
            {
                "id": 16,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 2
                    }
                ],
                "triggers": [
                    {
                        "kind": "water_detection_trigger",
                        "next_node_id": end_node,
                        "value": True
                    },
                    {
                        "kind": "water_detection_trigger",
                        "next_node_id": 12,
                        "value": False
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
                        "next_node_id": 16,
                        "operator": ">=",
                        "value": 2
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 13,
                        "next_node_id": -2,
                        "operator": ">=",
                        "value": 300
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

    return water_detection_stage
        

if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    prepurge_stage = get_water_detection_stage(json_parameters, 200, 1)
    print(json.dumps(prepurge_stage, indent=4))
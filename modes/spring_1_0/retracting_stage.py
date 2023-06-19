import json

def get_retracting_stage(parameters: json, start_stage: int, end_stage: int):
    return {}

import json

def get_retracting_stage(parameters: json,start_node: int, end_node: int):
    
    retracting_stage = {
        "name": "retracting",
        "nodes": [
            {
                "id": start_node,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "algorithm": "Piston Ease-In",
                        "direction": "UP",
                        "speed": 4
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 5,
                        "operator": "<=",
                        "value": -2,
                        "next_node_id": 9,
                        "source": "Piston Position Raw"
                    }
                ]
            },
            {
                "id": 9,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "algorithm": "Piston Ease-In",
                        "direction": "UP",
                        "speed": 6
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_speed_trigger",
                        "operator": "==",
                        "value": 0,
                        "next_node_id": 25
                    }
                ]
            },
            {
                "id": 25,
                "controllers": [
                    {
                        "kind": "tare_controller"
                    },
                    {
                        "kind": "time_reference",
                        "id": 10
                    }
                ],
                "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": 26
                    }
                ]
            },
            {
                "id": 26,
                "controllers": [
                    {
                        "kind": "weight_reference",
                        "id": 1
                    }
                ],
                "triggers": [
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 10,
                        "operator": ">=",
                        "value": 2,
                        "next_node_id": end_node
                    }
                ]
            }
        ]
    }
    
    # return retracting_stage
    return {}


if __name__ == '__main__':

    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)
    
    retracting_stage = get_retracting_stage(json_parameters,0, 1)
    print(json.dumps(retracting_stage, indent=4))
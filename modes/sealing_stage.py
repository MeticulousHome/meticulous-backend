import json

def get_sealing_stage(parameters: json,start_node: int, end_node: int):
    
    sealing_stage = {
        "name": "closing valve",
        "nodes": [
            {
                "id": start_node,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "algorithm": "Piston Ease-In",
                        "direction": "DOWN",
                        "speed": 5
                    },
                    {
                        "kind": "time_reference",
                        "id": 3
                    }
                ],
                "triggers": [
                    {
                        "kind": "pressure_value_trigger",
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 0.2,
                        "next_node_id": end_node
                    }
                ]
            }
        ]
    }
    
    return sealing_stage


if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    sealing_stage = get_sealing_stage(json_parameters, 200, 1)
    print(json.dumps(sealing_stage, indent=4))
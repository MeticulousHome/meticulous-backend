import json

def get_idle_stage(parameters: json,start_node: int, end_node: int):
    idle_stage = {
        "name": "idle",
        "nodes": [
            {
                "id": start_node,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 25
                    }
                ],
                "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": 35
                    }
                ]
            },
            {
                "id": 35,
                "controllers": [],
                "triggers": [
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 25,
                        "operator": ">=",
                        "value": 0.2,
                        "next_node_id": end_node
                    }
                ]
            }
        ]
    }
    
    return idle_stage


if __name__ == '__main__':
    
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    idle_stage = get_idle_stage(json_parameters,0, 1)
    print(json.dumps(idle_stage, indent=4))
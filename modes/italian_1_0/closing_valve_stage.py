import json

def get_closing_valve_stage(parameters: json,start_node: int, end_node: int):
    
    closing_valve_stage = {
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
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "operator": ">=",
                        "value": 78,
                        "next_node_id": -2,
                        "source": "Piston Position Raw"
                    }
                ]
            }
        ]
    }
    
    return closing_valve_stage
    # return {}


if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    closing_valve_stage = get_closing_valve_stage(json_parameters, 200, 1)
    print(json.dumps(closing_valve_stage, indent=4))
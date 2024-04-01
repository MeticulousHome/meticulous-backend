import json

def get_heating_stage(parameters: json, start_node: int, end_node: int):
    try:
        preheat = parameters['preheat']
    except:
        print('Warning: preheat is not defined')
    try:
        temperature = parameters['temperature']
    except:
        print('Error: temperature is not defined')
        return None
    
    control_algorithm = "Water Temperature PID v1.0"
    temperature_algorithm = temperature
    start_node_preheat = 8 if preheat else end_node
    
    heating_stage = {
        "name": "heating",
        "nodes": [
            {
            "id": start_node,
            "controllers": [
                {
                    "kind": "temperature_controller",
                    "algorithm": control_algorithm,
                    "curve": {
                        "id": 0,
                        "interpolation_kind": "linear_interpolation",
                        "points": [[0, temperature_algorithm]],
                        "reference": {
                            "kind": "time",
                            "id": 2,
                        },
                    },
                    },
                {
                    "kind": "position_reference",
                    "id": 5,
                }
            ],
            "triggers": [
                {
                    "kind": "temperature_value_trigger",
                    "source": "Tube Temperature",
                    "operator": ">=",
                    "value": temperature - 2,
                    "next_node_id": 6,
                },
                {
                    "kind": "timer_trigger",
                    "timer_reference_id": 2,
                    "operator": ">=",
                    "value": 900,
                    "next_node_id": -2,
                },
                {
                    "kind": "button_trigger",
                    "source": "Encoder Button",
                    "gesture": "Single Tap",
                    "next_node_id": 6,
                },
            ],
            },
            {
                "id": 6,
                "controllers": [
                {
                    "kind": "time_reference",
                    "id": 8
                }
                ],
                "triggers": [
                {
                    "kind": "exit",
                    "next_node_id": 7
                },
                {
                    "kind": "button_trigger",
                    "next_node_id": start_node_preheat,
                    "gesture": "Single Tap",
                    "source": "Encoder Button"
                }
                ]
            },
            {
                "id": 7,
                "controllers": [
                {
                    "kind": "time_reference",
                    "id": 9
                }
                ],
                "triggers": [
                {
                    "kind": "timer_trigger",
                    "timer_reference_id": 8,
                    "next_node_id": 10,
                    "operator": ">=",
                    "value": 1
                },
                {
                    "kind": "button_trigger",
                    "next_node_id": start_node_preheat,
                    "gesture": "Single Tap",
                    "source": "Encoder Button"
                }
                ]
            },
            {
                "id": 10,
                "controllers": [],
                "triggers": [
                {
                    "kind": "temperature_value_trigger",
                    "next_node_id": start_node_preheat,
                    "source": "Water Temperature",
                    "operator": ">=",
                    "value": temperature
                },
                {
                    "kind": "temperature_value_trigger",
                    "next_node_id": 6,
                    "source": "Water Temperature",
                    "operator": "<=",
                    "value": temperature - 2
                },
                {
                    "kind": "timer_trigger",
                    "timer_reference_id": 9,
                    "next_node_id": start_node_preheat,
                    "operator": ">=",
                    "value": 5
                },
                {
                    "kind": "button_trigger",
                    "next_node_id": start_node_preheat,
                    "gesture": "Single Tap",
                    "source": "Encoder Button"
                }
                ]
            },
        ],
    }
    
    if preheat:
        preheat_stage = {
            "name": "click to start",
            "nodes": [
                {
                "id": start_node_preheat,
                "controllers": [],
                "triggers": [
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": end_node,
                    },
                ],
                },
            ],
        }
    
    return heating_stage if not preheat else [heating_stage, preheat_stage]
    # return {}


if __name__ == '__main__':


    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    heating_stage = get_heating_stage(json_parameters, 0, 1)
    print(json.dumps(heating_stage, indent=4))
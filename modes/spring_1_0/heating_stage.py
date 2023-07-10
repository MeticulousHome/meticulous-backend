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
    
    preheat_next_node_id = end_node if not preheat else 8
    
    heating_stage = {
        "name": "heating",
        "nodes": [
            {
            "id": start_node,
            "controllers": [
                {
                    "kind": "temperature_controller",
                    "algorithm": "Cylinder Temperature PID v1.0",
                    "curve": {
                        "id": 0,
                        "interpolation_kind": "linear_interpolation",
                        "points": [[0, 180]],
                        "reference": {
                            "kind": "time",
                            "id": 2,
                        },
                    },
                    },
                {
                    "kind": "position_reference",
                    "id": 5,
                },
                {
                    "kind": "time_reference",
                    "id": 20,
                },
            ],
            "triggers": [
                {
                    "kind": "temperature_value_trigger",
                    "source": "Tube Temperature",
                    "operator": ">=",
                    "value": temperature,
                    "next_node_id": preheat_next_node_id,
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
                    "next_node_id": end_node,
                },
            ],
            },
        ],
    }
    
    if preheat:
        preheat_stage = {
            "name": "press dial",
            "nodes": [
                {
                "id": 8,
                "controllers": [
                    {
                    "kind": "temperature_controller",
                    "algorithm": "Water Temperature PID v1.0",
                    "curve": {
                        "id": 1,
                        "interpolation_kind": "linear_interpolation",
                        "points": [[0, temperature]],
                        "reference": {
                            "kind": "time",
                            "id": 2,
                        },
                    },
                    },
                ],
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
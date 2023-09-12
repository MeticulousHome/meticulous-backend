import json

def get_spring_stage(parameters: json,start_node: int, end_node: int):
    
    try:
        max_pressure = parameters['parameters']['max_pressure']
    except:
        print('Error: Max pressure is not defined')
        return None
    try:
        max_power = parameters['parameters']['max_power']
    except:
        print('Error: Max power is not defined')
        return None
    try:
        min_power = parameters['parameters']['min_power']
    except:
        print('Error: Min power is not defined')
        return None
    try:
        weight_trigger = parameters['parameters']['weight_trigger']
    except:
        print('Error: Weight trigger is not defined')
        return None
    
      
    spring_stage =     {
      "name": "spring",
      "nodes": [
        {
          "id": start_node,
          "controllers": [
            {
              "kind": "position_reference",
              "id": 8
            },
            {
              "kind": "time_reference",
              "id": 31
            },
            {
              "kind": "weight_reference",
              "id": 3
            }
          ],
          "triggers": [
            {
              "kind": "exit",
              "next_node_id": 39
            }
          ]
        },
        {
          "id": 39,
          "controllers": [
            {
              "kind": "piston_power_controller",
              "algorithm": "Spring v1.0",
              "curve": {
                "id": 16,
                "interpolation_kind": "catmull_interpolation",
                "points": [
                  [
                    0,
                    min_power
                  ],
                  [
                    60,
                    max_power
                  ]
                ],
                "reference": {
                  "kind": "position",
                  "id": 8
                }
              }
            }
          ],
          "triggers": [
            {
              "kind": "piston_position_trigger",
              "position_reference_id": 7,
              "operator": ">=",
              "value": 60,
              "next_node_id": end_node,
              "source": "Piston Position Raw"
            },
            {
              "kind": "pressure_value_trigger",
              "source": "Pressure Predictive",
              "operator": ">=",
              "value": max_pressure,
              "next_node_id": 41
            },
            {
              "kind": "weight_value_trigger",
              "source": "Weight Raw",
              "weight_reference_id": 1,
              "operator": ">=",
              "value": weight_trigger,
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
          "id": 41,
          "controllers": [
            {
              "kind": "pressure_controller",
              "algorithm": "Pressure PID v1.0",
              "curve": {
                "id": 18,
                "interpolation_kind": "linear_interpolation",
                "points": [
                  [
                    0,
                    max_pressure
                  ]
                ],
                "reference": {
                  "kind": "time",
                  "id": 31
                }
              }
            }
          ],
          "triggers": [
            {
              "kind": "piston_power_curve_trigger",
              "source": "Raw Piston Power",
              "operator": ">=",
              "curve_id": 16,
              "next_node_id": 39
            },
            {
              "kind": "piston_position_trigger",
              "position_reference_id": 7,
              "operator": ">=",
              "value": 60,
              "next_node_id": end_node,
              "source": "Piston Position Raw"
            },
            {
              "kind": "weight_value_trigger",
              "source": "Weight Raw",
              "weight_reference_id": 1,
              "operator": ">=",
              "value": weight_trigger,
              "next_node_id": end_node
            },
            {
              "kind": "button_trigger",
              "source": "Encoder Button",
              "gesture": "Single Tap",
              "next_node_id": end_node
            }
          ]
        }
      ]
    }
    
    return spring_stage
    # return {}


if __name__ == '__main__':
    with open('parameters.json') as json_file:
        json_parameters = json.load(json_file)

    spring_stage = get_spring_stage(json_parameters, 200, 1)
    print(json.dumps(spring_stage, indent=4))
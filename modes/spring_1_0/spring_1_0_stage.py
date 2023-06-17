import json

def get_spring_stage(parameters: json,start_node: int, end_node: int):
    
    spring_stage = {
      "name": "spring",
      "nodes": [
        {
          "id": 38,
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
                    50
                  ],
                  [
                    60,
                    90
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
              "next_node_id": 34,
              "source": "Piston Position Raw"
            },
            {
              "kind": "pressure_value_trigger",
              "source": "Pressure Predictive",
              "operator": ">=",
              "value": 11,
              "next_node_id": 41
            },
            {
              "kind": "weight_value_trigger",
              "source": "Weight Raw",
              "weight_reference_id": 1,
              "operator": ">=",
              "value": 45,
              "next_node_id": 34
            },
            {
              "kind": "button_trigger",
              "source": "Encoder Button",
              "gesture": "Single Tap",
              "next_node_id": 34
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
                    11
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
              "next_node_id": 34,
              "source": "Piston Position Raw"
            },
            {
              "kind": "weight_value_trigger",
              "source": "Weight Raw",
              "weight_reference_id": 1,
              "operator": ">=",
              "value": 45,
              "next_node_id": 34
            },
            {
              "kind": "button_trigger",
              "source": "Encoder Button",
              "gesture": "Single Tap",
              "next_node_id": 34
            }
          ]
        }
      ]
    }
    
    return spring_stage


if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    spring_stage = get_spring_stage(json_parameters, 200, 1)
    print(json.dumps(spring_stage, indent=4))
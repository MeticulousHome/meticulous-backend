import json

def get_end_purge_stage(parameters: json, start_node: int, end_node: int):
    end_purge_stage =      {
      "name": "purge",
      "nodes": [
       {
        "id": start_node,
        "controllers": [
         {
          "kind": "move_piston_controller",
          "algorithm": "Piston Ease-In",
          "direction": "DOWN",
          "speed": 6
         },
         {
          "kind": "time_reference",
          "id": 1
         }
        ],
        "triggers": [
         {
          "kind": "pressure_value_trigger",
          "source": "Pressure Raw",
          "operator": ">=",
          "value": 6,
          "next_node_id": 17
         },
         {
          "kind": "piston_position_trigger",
          "position_reference_id": 0,
          "source": "Piston Position Raw",
          "operator": ">=",
          "value": 60,
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
        "id": 17,
        "controllers": [
         {
          "kind": "pressure_controller",
          "algorithm": "Pressure PID v1.0",
          "curve": {
           "id": 20,
           "interpolation_kind": "linear_interpolation",
           "points": [
            [
             0,
             6
            ]
           ],
           "time_reference_id": 1
          }
         }
        ],
        "triggers": [
         {
          "kind": "piston_position_trigger",
          "position_reference_id": 0,
          "source": "Piston Position Raw",
          "operator": ">=",
          "value": 60,
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
    
    # return end_purge_stage
    return {}


if __name__ == '__main__':
    
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    end_purge_stage = get_end_purge_stage(json_parameters,0, 1)
    print(json.dumps(end_purge_stage, indent=4))
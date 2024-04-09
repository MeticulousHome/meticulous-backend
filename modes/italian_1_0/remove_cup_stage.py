import json

def get_remove_cup_stage(parameters: json, start_node: int, end_node: int):
    
    remove_cup_stage =  {
      "name": "remove cup",
      "nodes": [
       {
        "id": start_node,
        "controllers": [
         {
          "kind": "time_reference",
          "id": 5
         }
        ],
        "triggers": [
         {
          "kind": "weight_value_trigger",
          "source": "Weight Raw",
          "weight_reference_id": 2,
          "operator": "<=",
          "value": -5,
          "next_node_id": 30
         },
         {
          "kind": "button_trigger",
          "source": "Encoder Button",
          "gesture": "Single Tap",
          "next_node_id": end_node,
        },
        ]
       },
       {
        "id": 30,
        "controllers": [],
        "triggers": [
         {
          "kind": "timer_trigger",
          "timer_reference_id": 5,
          "operator": ">=",
          "value": 5,
          "next_node_id": end_node
         }
        ]
       }
      ]
     }
    
    return remove_cup_stage
    # return {}


if __name__ == '__main__':
    
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    remove_cup_stage = get_remove_cup_stage(json_parameters,0, 1)
    print(json.dumps(remove_cup_stage, indent=4))
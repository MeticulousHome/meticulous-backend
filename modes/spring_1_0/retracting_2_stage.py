import json

def get_retracting_2_stage(parameters: json,start_node: int, end_node: int):
    
    retracting_2_stage = {
         "name": "retracting",
         "nodes": [
           {
             "id": start_node,
             "controllers": [
               {
                 "kind": "weight_reference",
                 "id": 2
               },
               {
                 "kind": "time_reference",
                 "id": 17
               }
             ],
             "triggers": [
               {
                 "kind": "exit",
                 "next_node_id": 14
               }
             ]
           },
           {
             "id": -2,
             "controllers": [
               {
                 "kind": "end_profile"
               }
             ],
             "triggers": []
           },
           {
             "id": 14,
             "controllers": [
               {
                 "kind": "move_piston_controller",
                 "algorithm": "Piston Fast",
                 "direction": "UP",
                 "speed": 4.0
               },
               {
                 "kind": "time_reference",
                 "id": 18
               }
             ],
             "triggers": [
               {
                 "kind": "piston_position_trigger",
                 "position_reference_id": 1,
                 "operator": "<=",
                 "value": -4.0,
                 "next_node_id": 15,
                 "source": "Piston Position Raw"
               },
               {
                 "kind": "timer_trigger",
                 "timer_reference_id": 17,
                 "operator": ">=",
                 "value": 5,
                 "next_node_id": 15
               }
             ]
           },
           {
             "id": 15,
             "controllers": [
               {
                 "kind": "move_piston_controller",
                 "algorithm": "Piston Fast",
                 "direction": "UP",
                 "speed": 6.0
               }
             ],
             "triggers": [
               {
                 "kind": "timer_trigger",
                 "timer_reference_id": 18,
                 "operator": ">=",
                 "value": 3,
                 "next_node_id": 32
               }
             ]
           },
           {
             "id": 32,
             "controllers": [],
             "triggers": [
               {
                 "kind": "piston_speed_trigger",
                 "operator": "==",
                 "value": 0,
                 "next_node_id": end_node
               }
             ]
           }
         ]
       } 
    
    return retracting_2_stage
    # return {}


if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    retracting_2_stage = get_retracting_2_stage(json_parameters, 200, 1)
    print(json.dumps(retracting_2_stage, indent=4))
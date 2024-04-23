import json

def get_infusion_stage(parameters: json, start_node: int, end_node: int):
    
    try:
        out_weight = parameters["out_weight"]
    except:
        print('Error: Out weight is not defined')
        return None
    
    try:
        pressure = parameters["pressure"]
    except:
        print('Error: Pressure is not defined')
        return None
    
    infusion_stage =   {
        "name": "infusion",
        "nodes": [
            {
                "id": start_node,
                "controllers": [
                 {
                  "kind": "time_reference",
                  "id": 4
                 }
                ],
                "triggers": [
                 {
                  "kind": "exit",
                  "next_node_id": 13
                 }
                ]
            },
            {
             "id": 13,
             "controllers": [
              {
               "kind": "temperature_controller",
               "algorithm": "Cylinder Temperature PID v1.0",
               "curve": {
                "id": 12,
                "interpolation_kind": "linear_interpolation",
                "points": [
                 [
                  0,
                  25
                 ]
                ],
                "time_reference_id": 4
               }
              },
              {
               "kind": "pressure_controller",
               "algorithm": "Pressure PID v1.0",
               "curve": {
                "id": 7,
                "interpolation_kind": "catmull_interpolation",
                "points": [
                 [
                  0,
                  pressure
                 ]
                ],
                "time_reference_id": 4
               }
              },
              {
               "kind": "position_reference",
               "id": 1
              }
             ],
             "triggers": [
              {
               "kind": "timer_trigger",
               "timer_reference_id": 4,
               "operator": ">=",
               "value": 100,
               "next_node_id": end_node
              },
              {
               "kind": "weight_value_trigger",
               "source": "Weight Raw",
               "weight_reference_id": 1,
               "operator": ">=",
               "value": out_weight,
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
    return infusion_stage
    # return {}
    
if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200, "out_weight": 0.3, "pressure": 8}'

    json_parameters = json.loads(parameters)

    infusion_stage = get_infusion_stage(json_parameters, 200, 1)
    print(json.dumps(infusion_stage, indent=4))
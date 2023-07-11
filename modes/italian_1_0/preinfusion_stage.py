import json

def get_preinfusion_stage(parameters: json, start_node: int, end_node: int):
    
    preinfusion_stage =   {
      "name": "preinfusion",
      "nodes": [
       {
        "id": start_node,
        "controllers": [
         {
          "kind": "temperature_controller",
          "algorithm": "Cylinder Temperature PID v1.0",
          "curve": {
           "id": 3,
           "interpolation_kind": "linear_interpolation",
           "points": [
            [
             0,
             25
            ]
           ],
           "time_reference_id": 2
          }
         },
         {
          "kind": "flow_controller",
          "algorithm": "Flow PID v1.0",
          "curve": {
           "id": 4,
           "interpolation_kind": "catmull_interpolation",
           "points": [
            [
             0,
             4
            ]
           ],
           "time_reference_id": 3
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
          "timer_reference_id": 3,
          "operator": ">=",
          "value": 30,
          "next_node_id": end_node
         },
         {
          "kind": "pressure_value_trigger",
          "source": "Pressure Predictive",
          "operator": ">=",
          "value": 8,
          "next_node_id": 11
         },
         {
          "kind": "weight_value_trigger",
          "source": "Weight Raw",
          "weight_reference_id": 1,
          "operator": ">=",
          "value": 0.3,
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
        "id": 11,
        "controllers": [
         {
          "kind": "pressure_controller",
          "algorithm": "Pressure PID v1.0",
          "curve": {
           "id": 6,
           "interpolation_kind": "catmull_interpolation",
           "points": [
            [
             0,
             8
            ]
           ],
           "time_reference_id": 3
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
          "timer_reference_id": 3,
          "operator": ">=",
          "value": 30,
          "next_node_id": end_node
         },
         {
          "kind": "flow_curve_trigger",
          "source": "Flow Raw",
          "operator": ">=",
          "curve_id": 4,
          "next_node_id": 10
         },
         {
          "kind": "weight_value_trigger",
          "source": "Weight Raw",
          "weight_reference_id": 1,
          "operator": ">=",
          "value": 0.3,
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
    return preinfusion_stage
    # return {}

if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    preinfusion_stage = get_preinfusion_stage(json_parameters, 200, 1)
    print(json.dumps(preinfusion_stage, indent=4))
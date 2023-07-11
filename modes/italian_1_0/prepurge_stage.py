import json

def get_prepurge_stage(parameters: json, start_node: int, end_node: int):
    return {}

import json

def get_prepurge_stage(parameters: json,start_node: int, end_node: int):
    
    prepurge_stage = {
        "name": "purge",
        "nodes": [
            {
                "id": start_node,
                "controllers": [],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "operator": ">=",
                        "value": 60,
                        "next_node_id": 5,
                        "source": "Piston Position Raw"
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "operator": "<",
                        "value": 60,
                        "next_node_id": 2,
                        "source": "Piston Position Raw"
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": 5
                    }
                ]
            },
            {
                "id": 2,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "algorithm": "Piston Ease-In",
                        "direction": "DOWN",
                        "speed": 6.0
                    },
                    {
                        "kind": "time_reference",
                        "id": 30
                    }
                ],
                "triggers": [
                    {
                        "kind": "pressure_value_trigger",
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 6.0,
                        "next_node_id": 3
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 60,
                        "next_node_id": 5
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": 5
                    }
                ]
            },
            {
                "id": 3,
                "controllers": [
                    {
                        "kind": "pressure_controller",
                        "algorithm": "Pressure PID v1.0",
                        "curve": {
                            "id": 13,
                            "interpolation_kind": "linear_interpolation",
                            "points": [
                                [
                                    0,
                                    6
                                ]
                            ],
                            "time_reference_id": 30
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
                        "next_node_id": 5
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": 5
                    }
                ]
            },
        ]
    }
    
    # This code is ready when water detection will be implemented
    
    #     water_detection_node = {
    #     "id": 5,
    #     "controllers": [
    #         {
    #             "kind": "time_reference",
    #             "id": 2
    #         }
    #     ],
    #     "triggers": [
    #         {
    #             "kind": "water_detection_trigger",
    #             "value": True,
    #             "next_node_id": 7
    #         },
    #         {
    #             "kind": "timer_trigger",
    #             "timer_reference_id": 2,
    #             "operator": ">=",
    #             "value": 100,
    #             "next_node_id": 6
    #         }
    #     ]
    # }
        
    water_detection_node = {
                "id": 5,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 2
                    },
                    {
                        "kind": "move_piston_controller",
                        "algorithm": "Piston Ease-In",
                        "direction": "DOWN",
                        "speed": 0.0
                    }
                ],
                "triggers": [
                    {
                        "kind": "water_detection_trigger",
                        "value": True,
                        "next_node_id": end_node
                    },
                ]
            }
    
    no_water_node = {
                "id": 6,
                "controllers": [
                    {
                        "kind": "log_controller",
                        "message": "No Water"
                    }
                ],
                "triggers": [
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 2,
                        "operator": ">=",
                        "value": 2,
                        "next_node_id": 5
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 1,
                        "operator": ">=",
                        "value": 100,
                        "next_node_id": -2
                    }
                ]
    }
    water_detection_value = water_detection_node['triggers'][0]['value']

    # Set next_node_id based on the water_detection_value
    end_node = end_node if water_detection_value else 6
    water_detection_node['triggers'][0]['next_node_id'] = end_node

    # Merge the dictionaries
    if water_detection_value:
        # Merge pre_purge_stage with water_detection_node
        prepurge_stage['nodes'].append(water_detection_node)
    else:
        # Merge pre_purge_stage with water_detection_node and no_water_node
        prepurge_stage['nodes'].extend([water_detection_node, no_water_node])

    # The merged dictionary is now in pre_purge_stage
    return prepurge_stage
    # return {}
    



if __name__ == '__main__':
  
    parameters = '{"preheat": true,"temperature": 200}'

    json_parameters = json.loads(parameters)

    prepurge_stage = get_prepurge_stage(json_parameters, 200, 1)
    print(json.dumps(prepurge_stage, indent=4))
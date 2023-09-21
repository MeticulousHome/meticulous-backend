import json

def get_curve_stage(parameters: json, start_node: int, end_node: int):
    stages_list = []
    values = parameters["stages"]
    increment_node = 1000 
    
    # Valores iniciales para los IDs de las curvas
    temperature_curve_id = 101
    secondary_curve_id = 547
    primary_curve_id = 929
    main_node_id = 13
    secondary_node_id = 20
    
    # Usamos enumerate para obtener el índice actual del bucle
    for index, stage in enumerate(values):
        if stage["kind"] == "curve_1.0":
            stages_list.append(get_curve(stage, start_node, end_node, temperature_curve_id, secondary_curve_id, primary_curve_id, main_node_id, secondary_node_id, index, increment_node, len(values)))
        elif stage["kind"] == "spring_1.0":
            stages_list.append(get_spring())
        
        # Incremento de los IDs para la siguiente iteración
        temperature_curve_id += 1
        secondary_curve_id += 1
        primary_curve_id += 1
        main_node_id += 1
        secondary_node_id += 1
        start_node += increment_node

    return stages_list

def get_curve(parameters, start_node, end_node, temperature_curve_id, secondary_curve_id, primary_curve_id, main_node_id, secondary_node_id, index, increment_node, total_stages):
    # Extracting necessary data from parameters
    name = parameters["name"].lower()
    points_controller = parameters["parameters"]["points"]
    interpolation_kind = parameters["parameters"]["interpolation_method"] + "_interpolation"

    max_time = 0
    stop_weight = 0
    max_limit_trigger = 0
 

    for trigger in parameters["triggers"]:
        if trigger["kind"] == "time":
            max_time = trigger["value"]
        elif trigger["kind"] == "weight":
            stop_weight = trigger["value"]

    for limit in parameters["limits"]:
        if limit["kind"] == "pressure":
            max_limit_trigger = limit["value"]
        elif limit["kind"] == "flow":
            max_limit_trigger = limit["value"]
            
    interpolation_kind = parameters["parameters"]["interpolation_method"] + "_interpolation"
    
        # Determining control kinds and algorithms
    if parameters["parameters"]["control_method"] == "pressure":
        control_kind = "pressure_controller"
        algorithm = "Pressure PID v1.0"
        press_flow_triger = "flow_value_trigger"
        trigger_source = "Flow Raw"
        control_kind_secondary = "flow_controller"
        algorithm_secondary = "Flow PID v1.0"
        pressure_flow_curve_trigger = "pressure_curve_trigger"
        curve_trigger_source = "Pressure Raw"
    else:
        control_kind = "flow_controller"
        algorithm = "Flow PID v1.0"
        press_flow_triger = "pressure_value_trigger"
        trigger_source = "Pressure Raw"
        control_kind_secondary = "pressure_controller"
        algorithm_secondary = "Pressure PID v1.0"
        pressure_flow_curve_trigger =  "flow_curve_trigger"
        curve_trigger_source = "Flow Raw"
        
    if index == total_stages - 1:
        next_end_node = end_node
    else:
        next_end_node = start_node + increment_node
        
    curve_template ={
            "name": name,
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
                            "next_node_id": main_node_id
                        }
                    ]
                },
                {
                    "id": main_node_id,
                    "controllers": [
                        {
                            "kind": "temperature_controller",
                            "algorithm": "Cylinder Temperature PID v1.0",
                            "curve": {
                                "id": temperature_curve_id,
                                "interpolation_kind": "linear_interpolation",
                                "points": [
                                    [0, 25.0]
                                ],
                        "reference": {
                            "kind": "time",
                            "id": 4
                        }
                            }
                        },
                        {
                            "kind": control_kind,
                            "algorithm": algorithm,
                            "curve": {
                                "id": primary_curve_id,
                                "interpolation_kind": interpolation_kind,
                                "points": points_controller,
                        "reference": {
                            "kind": "time",
                            "id": 4
                        }
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
                            "value": max_time,
                            "next_node_id": next_end_node
                        },
                        {
                            "kind": "weight_value_trigger",
                            "source": "Weight Raw",
                            "weight_reference_id": 1,
                            "operator": ">=",
                            "value": stop_weight,
                            "next_node_id": next_end_node
                        },
                        {
                            "kind": press_flow_triger,
                            "source": trigger_source,
                            "operator": ">=",
                            "value": max_limit_trigger,
                            "next_node_id": secondary_node_id
                        },
                        {
                            "kind": "button_trigger",
                            "source": "Encoder Button",
                            "gesture": "Single Tap",
                            "next_node_id": next_end_node
                        }
                    ]
                },
                {
                    "id": secondary_node_id,
                    "controllers": [
                        {
                            "kind": control_kind_secondary,
                            "algorithm": algorithm_secondary,
                            "curve": {
                                "id": secondary_curve_id,
                                "interpolation_kind": "catmull_interpolation",
                                "points": [
                                    [0, max_limit_trigger]
                                ],
                        "reference": {
                            "kind": "time",
                            "id": 4
                        }
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
                            "value": max_time,
                            "next_node_id": next_end_node
                        },
                        {
                            "kind": "weight_value_trigger",
                            "source": "Weight Raw",
                            "weight_reference_id": 1,
                            "operator": ">=",
                            "value": stop_weight,
                            "next_node_id": next_end_node
                        },
                        {
                            "kind": pressure_flow_curve_trigger,
                            "source": curve_trigger_source,
                            "operator": ">=",
                            "curve_id": primary_curve_id,
                            "next_node_id": next_end_node
                        },
                        {
                            "kind": "button_trigger",
                            "source": "Encoder Button",
                            "gesture": "Single Tap",
                            "next_node_id": next_end_node
                        }
                    ]
                }
            ]
        }
    return curve_template
        
        
def get_spring():
    # Mock JSON for spring
    return {
        "name": "Mock Spring",
        "kind": "spring"
    }
    
if __name__ == "__main__":
    # JSON de prueba

    data = {
        "name": "Dash",
        "kind": "dashboard_1_0",
        "temperature": 89,
        "preheat": True,
        "source": "dashboard",
        "action": "to_play",
        "stages": [
            {
                "parameters": {
                    "control_method": "pressure",
                    "interpolation_method": "linear",
                    "points": [[0, 4], [9, 9], [20, 9]]
                },
                "triggers": [
                    {"kind": "weight", "value": 0.3},
                    {"kind": "time", "value": 20}
                ],
                "limits": [
                    {"kind": "flow", "value": 6}
                ],
                "kind": "spring_1.0",
                "name": "Preinfusion"
            },
            {
                "parameters": {
                    "control_method": "flow",
                    "interpolation_method": "catmull",
                    "points": [[0, 0], [9, 8], [40, 8]]
                },
                "triggers": [
                    {"kind": "weight", "value": 33},
                    {"kind": "time", "value": 40}
                ],
                "limits": [
                    {"kind": "pressure", "value": 8}
                ],
                "kind": "spring_1.0",
                "name": "Infusion"
            }
        ]
    }

    # Llamamos a la función con el JSON de prueba
    result = get_curve_stage(data, 300, 301)
    
    # Imprimimos el resultado
    for stage in result:
        print(json.dumps(stage, indent=4))
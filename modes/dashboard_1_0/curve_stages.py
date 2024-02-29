from .spring_1_0 import get_spring
import json
class IDGenerator:
    def __init__(self, start_value):
        self.current_value = start_value

    def get_next(self):
        self.current_value += 1
        return self.current_value


node_id_generator = IDGenerator(6000)
position_id_generator = IDGenerator(6100)
time_id_generator = IDGenerator(6200)
curve_id_generator = IDGenerator(6300)

def get_curve_stage(parameters: json, start_node: int, end_node: int):
    stages_list = []
    values = parameters["stages"]
    
    # init first and second node to be persistend between stages
    FIRST_NODE_ID = 0
    SECOND_NODE_ID = 0
    # Usamos enumerate para obtener el índice actual del bucle
    for index, stage in enumerate(values):
        if index == 0:
            FIRST_NODE_ID = start_node
        else:
            FIRST_NODE_ID = SECOND_NODE_ID
        if index == len(values) - 1:
            SECOND_NODE_ID = end_node
        else:
            SECOND_NODE_ID = node_id_generator.get_next()

        if stage["kind"] == "curve_1.0":
            stages_list.append(get_curve(stage, FIRST_NODE_ID, SECOND_NODE_ID))
        elif stage["kind"] == "spring_1.0":
            stages_list.append(get_spring(stage, FIRST_NODE_ID, SECOND_NODE_ID))

    return stages_list

def get_curve(parameters, start_node, end_node):
    # Extracting necessary data from parameters
    name = parameters["name"].lower()
    points_controller = parameters["parameters"]["points"]
    interpolation_kind = parameters["parameters"]["interpolation_method"] + "_interpolation"

    max_time = 0
    stop_weight = 0
    max_limit_trigger = 0
 
    _NODE_1_ID = node_id_generator.get_next()
    _NODE_2_ID = node_id_generator.get_next()
    _CURVE_TEMP_ID = curve_id_generator.get_next()
    _CURVE_MAIN_ID = curve_id_generator.get_next()
    _CURVE_SECOND_ID = curve_id_generator.get_next()

    _TIME_REFERENCE_1_ID = time_id_generator.get_next()
    _POSITION_REFERENCE_1_ID = position_id_generator.get_next()

    INIT_NODE_ID = start_node
    END_NODE_ID = end_node

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
        
    # if index == total_stages - 1:
    #     next_end_node = end_node
    # else:
    #     next_end_node = start_node + increment_node
        
    curve_template ={
            "name": name,
            "nodes": [
                {
                    "id": INIT_NODE_ID,
                    "controllers": [
                        {
                            "kind": "time_reference",
                            "id": _TIME_REFERENCE_1_ID
                        }
                    ],
                    "triggers": [
                        {
                            "kind": "exit",
                            "next_node_id": _NODE_1_ID
                        }
                    ]
                },
                {
                    "id": _NODE_1_ID,
                    "controllers": [
                        {
                            "kind": control_kind,
                            "algorithm": algorithm,
                            "curve": {
                                "id": _CURVE_MAIN_ID,
                                "interpolation_kind": interpolation_kind,
                                "points": points_controller,
                                "reference": {
                                    "kind": "time",
                                    "id": _TIME_REFERENCE_1_ID
                                }
                            }
                        }
                    ],
                    "triggers": [
                        {
                            "kind": "timer_trigger",
                            "timer_reference_id": _TIME_REFERENCE_1_ID,
                            "operator": ">=",
                            "value": max_time,
                            "next_node_id": END_NODE_ID
                        },
                        {
                            "kind": "weight_value_trigger",
                            "source": "Weight Raw",
                            "weight_reference_id": 1,
                            "operator": ">=",
                            "value": stop_weight,
                            "next_node_id": END_NODE_ID
                        },
                        {
                            "kind": press_flow_triger,
                            "source": trigger_source,
                            "operator": ">=",
                            "value": max_limit_trigger,
                            "next_node_id": _NODE_2_ID
                        },
                        {
                            "kind": "button_trigger",
                            "source": "Encoder Button",
                            "gesture": "Single Tap",
                            "next_node_id": END_NODE_ID
                        }
                    ]
                },
                {
                    "id": _NODE_2_ID,
                    "controllers": [
                        {
                            "kind": control_kind_secondary,
                            "algorithm": algorithm_secondary,
                            "curve": {
                                "id": _CURVE_SECOND_ID,
                                "interpolation_kind": "catmull_interpolation",
                                "points": [
                                    [0, max_limit_trigger]
                                ],
                                "reference": {
                                    "kind": "time",
                                    "id": _TIME_REFERENCE_1_ID
                                }
                            }
                        }
                    ],
                    "triggers": [
                        {
                            "kind": "timer_trigger",
                            "timer_reference_id": _TIME_REFERENCE_1_ID,
                            "operator": ">=",
                            "value": max_time,
                            "next_node_id": END_NODE_ID
                        },
                        {
                            "kind": "weight_value_trigger",
                            "source": "Weight Raw",
                            "weight_reference_id": 1,
                            "operator": ">=",
                            "value": stop_weight,
                            "next_node_id": END_NODE_ID
                        },
                        {
                            "kind": pressure_flow_curve_trigger,
                            "source": curve_trigger_source,
                            "operator": ">=",
                            "curve_id": _CURVE_MAIN_ID,
                            "next_node_id": _NODE_1_ID
                        },
                        {
                            "kind": "button_trigger",
                            "source": "Encoder Button",
                            "gesture": "Single Tap",
                            "next_node_id": END_NODE_ID
                        }
                    ]
                }
            ]
        }
    return curve_template
              
    
if __name__ == "__main__":
    # JSON de prueba

    data = {
    "name": "Dash",
    "kind": "dashboard_1_0",
    "temperature": 89,
    "preheat": "false",
    "source": "dashboard",
    "action": "to_play",
    "stages": [
        {
            "parameters": {
                "max_power": 25,
                "min_power": 35
            },
            "triggers": [
                {"kind": "weight", "value": 0.3}
            ],
            "limits": [
                {"kind": "pressure", "value": 6}
            ],
            "kind": "spring_1.0",
            "name": "Spring"
        },
        {
            "parameters": {
                "control_method": "pressure",
                "interpolation_method": "catmull",
                "points": [[0, 0], [7, 7], [40, 7]]
            },
            "triggers": [
                {"kind": "weight", "value": 33},
                {"kind": "time", "value": 40}
            ],
            "limits": [
                {"kind": "flow", "value": 8}
            ],
            "kind": "curve_1.0",
            "name": "Infusion"
        },
        {
            "parameters": {
                "control_method": "pressure",
                "interpolation_method": "catmull",
                "points": [[0, 1], [7, 2], [40, 3]]
            },
            "triggers": [
                {"kind": "weight", "value": 34},
                {"kind": "time", "value": 41}
            ],
            "limits": [
                {"kind": "flow", "value": 9}
            ],
            "kind": "curve_1.0",
            "name": "Infusion"
        },
        {
            "parameters": {
                "max_power": 26,
                "min_power": 36
            },
            "triggers": [
                {"kind": "weight", "value": 0.4}
            ],
            "limits": [
                {"kind": "pressure", "value": 7}
            ],
            "kind": "spring_1.0",
            "name": "Spring"
        },
    ]
}

    # Llamamos a la función con el JSON de prueba
    result = get_curve_stage(data, 300, 301)
    
    # Imprimimos el resultado
    for stage in result:
        print(json.dumps(stage, indent=4))
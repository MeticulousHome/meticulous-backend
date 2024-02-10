import json

class HeadProfile:
    
    def __init__(self):
        self.data = {}
        # try:
        #     self.click_to_start = click_to_start
        # except:
        #     self.click_to_start = True
        #     print('Warning: click_to_start is not defined, defaulting to True')   
            
    def purge_stage(self):
        self.data = {
            "name": "purge",
            "nodes": [
                {
                "id": -1,
                "controllers": [],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": 45,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 78
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": 6,
                        "source": "Piston Position Raw",
                        "operator": "<",
                        "value": 78
                    }
                ]
                },
                {
                "id": 6,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "speed": 6,
                        "direction": "DOWN",
                        "algorithm": "Piston Ease-In"
                    },
                    {
                        "kind": "time_reference",
                        "id": 3
                    }
                ],
                "triggers": [
                    {
                        "kind": "pressure_value_trigger",
                        "next_node_id": 11,
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 6
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": 45,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 78
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
                            "id": 1,
                            "interpolation_kind": "linear_interpolation",
                            "points": [
                            [
                                0,
                                6
                            ]
                            ],
                            "reference": {
                            "kind": "time",
                            "id": 3
                            }
                        }
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": 45,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": 78
                    }
                ]
                }
            ]
         }
        return self.data
    
    def water_detection_stage(self, water_detection: bool):
        self.data = {
            "name": "water detection",
            "nodes": [
                {
                "id": 12,
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
                        "next_node_id": 9,
                        "operator": ">=",
                        "value": 2
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 12,
                        "next_node_id": -2,
                        "operator": ">=",
                        "value": 100
                    }
                ]
                },
                {
                "id": 45,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 12
                    }
                ],
                "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": 9
                    }
                ]
                },
                {
                "id": 9,
                "controllers": [
                    {
                        "kind": "time_reference",
                        "id": 2
                    }
                ],
                "triggers": [
                    {
                        "kind": "water_detection_trigger",
                        "next_node_id": 15,
                        "value": water_detection
                    },
                    {
                        "kind": "water_detection_trigger",
                        "next_node_id": 12,
                        "value": water_detection
                    }
                ]
                }
            ]
        }
        return self.data

    def heating_stage(self,target_temperature: float, click_to_start: bool):
        if  click_to_start:
            self.next_node_id = 16
        else:
            self.next_node_id = 17
        
        self.data = {
            "name": "heating",
            "nodes": [
                {
                "id": 15,
                "controllers": [
                    {
                        "kind": "temperature_controller",
                        "algorithm": "Water Temperature PID v1.0",
                        "curve": {
                            "id": 2,
                            "interpolation_kind": "linear_interpolation",
                            "points": [
                            [
                                0,
                                target_temperature
                            ]
                            ],
                            "reference": {
                            "kind": "time",
                            "id": 2
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
                        "kind": "temperature_value_trigger",
                        "next_node_id": self.next_node_id,
                        "source": "Water Temperature",
                        "operator": ">=",
                        "value": target_temperature
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 2,
                        "next_node_id": 14,
                        "operator": ">=",
                        "value": 900
                    }
                ]
                }
            ]
        }
        
        self.click_to_start_stage = {
            "name": "click to start",
            "nodes": [
                {
                    "id": 16,
                    "controllers": [
                        {
                            "kind": "log_controller",
                            "message": "Click to start"
                        }
                    ],
                    "triggers": [
                        {
                            "kind": "button_trigger",
                            "next_node_id": 17,
                            "source": "Encoder Button"
                        }
                    ]
                }
            ]
        }
        
        if click_to_start:
            
            return [self.data, self.click_to_start_stage]  
        else:
            return self.data
            
    def retracting_stage(self):
        self.data = {
            "name": "retracting",
            "nodes": [
                {
                "id": 18,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "speed": 6,
                        "direction": "UP",
                        "algorithm": "Piston Ease-In"
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_speed_trigger",
                        "next_node_id": 21,
                        "operator": "==",
                        "value": 0
                    }
                ]
                },
                {
                "id": 21,
                "controllers": [
                    {
                        "kind": "tare_controller"
                    },
                    {
                        "kind": "time_reference",
                        "id": 4
                    }
                ],
                "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": 22
                    }
                ]
                },
                {
                "id": 22,
                "controllers": [
                    {
                        "kind": "weight_reference",
                        "id": 1
                    }
                ],
                "triggers": [
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 4,
                        "next_node_id": 23,
                        "operator": ">=",
                        "value": 2
                    }
                ]
                },
                {
                "id": 17,
                "controllers": [
                    {
                        "kind": "move_piston_controller",
                        "speed": 4,
                        "direction": "UP",
                        "algorithm": "Piston Ease-In"
                    }
                ],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "next_node_id": 18,
                        "source": "Piston Position Raw",
                        "position_reference_id": 1,
                        "operator": "<=",
                        "value": -2
                    }
                ]
                }
            ] 
        }
        return self.data
    
    def closing_valve_stage(self, end_node: int):
        self.data = {
            "name": "closing valve",
            "nodes": [
                {
                "id": 23,
                "controllers": [
                    {
                        "kind": "temperature_controller",
                        "algorithm": "Cylinder Temperature PID v1.0",
                        "curve": {
                            "id": 6,
                            "interpolation_kind": "linear_interpolation",
                            "points": [
                            [
                                0,
                                25
                            ]
                            ],
                            "reference": {
                            "kind": "time",
                            "id": 9
                            }
                        }
                    },
                    {
                        "kind": "move_piston_controller",
                        "speed": 5,
                        "direction": "DOWN",
                        "algorithm": "Piston Ease-In"
                    },
                    {
                        "kind": "time_reference",
                        "id": 1
                    }
                ],
                "triggers": [
                    {
                        "kind": "pressure_value_trigger",
                        "next_node_id": end_node,
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 0.2
                    }
                ]
                }
            ]
        }   
        return self.data 

    
    
if __name__ == '__main__':
    
    head_profile = HeadProfile()
    print(json.dumps(head_profile.purge_stage(), indent=4))
    print(json.dumps(head_profile.water_detection_stage(True), indent=4))
    print(json.dumps(head_profile.heating_stage(200, False), indent=4))
    print(json.dumps(head_profile.retracting_stage(), indent=4))
    print(json.dumps(head_profile.closing_valve_stage(11000), indent=4))
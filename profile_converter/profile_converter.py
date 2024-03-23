import json
from .profile_json import *
from .simplified_json import *


class ComplexProfileConverter:
    
    def __init__(self, click_to_start: bool, click_to_purge: bool, end_node_head: int, init_node_tail: int, parameters : dict = None):
        
        self.data = None
        self.parameters = parameters if parameters is not None else {}
        self.click_to_start = click_to_start if click_to_start is not None else True
        self.click_to_purge = click_to_purge if click_to_purge is not None else True
        self.end_node_head = end_node_head
        self.init_node_tail = init_node_tail
        self.complex = SimplifiedJson(self.parameters)
        self.temperature = self.complex.get_temperature() 
        self.offset_temperature = 1
        self.max_piston_position = 82
        
    def head_template(self):
        if self.click_to_start:
            self.head_next_node_id = 13
        else:
            self.head_next_node_id = 13
        self.stages_head = [
        {
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
                        "value": self.max_piston_position 
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": 6,
                        "source": "Piston Position Raw",
                        "operator": "<",
                        "value": self.max_piston_position 
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 45,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
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
                        "value": self.max_piston_position 
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 45,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
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
                        "value": self.max_piston_position 
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 45,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
                    }
                ]
                }
            ]
        },
        {
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
                        "value": 300
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 15,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
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
                        "value": True
                    },
                    {
                        "kind": "water_detection_trigger",
                        "next_node_id": 12,
                        "value": False
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 15,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
                    }
                ]
                }
            ]
        },
        {   
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
                                self.temperature 
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
                        "next_node_id": 7,
                        "source": "Water Temperature",
                        "operator": ">=",
                        "value": self.temperature - self.offset_temperature
                    },
                    {
                        "kind": "timer_trigger",
                        "timer_reference_id": 2,
                        "next_node_id": -2,
                        "operator": ">=",
                        "value": 900
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 7,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
                    }
                ]
                }
            ]
        },
        {
            "name": "heating",
            "nodes": [
                {
                    "id": 5,
                    "controllers": [
                        {
                            "kind": "time_reference",
                            "id": 3
                        }    
                    ],
                    "triggers": [
                        {
                            "kind": "exit",
                            "next_node_id": 7
                        },
                        {
                            "kind": "button_trigger",
                            "next_node_id": self.head_next_node_id,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                        }
                    ]   
                },
                {
                    "id": 7,
                    "controllers": [
                        {
                            "kind": "time_reference",
                            "id": 5
                        } 
                    ],
                    "triggers": [
                        {
                            "kind": "timer_trigger",
                            "timer_reference_id": 3,
                            "next_node_id": 8,
                            "operator": ">=",
                            "value": 1
                        },
                        {
                            "kind": "button_trigger",
                            "next_node_id": self.head_next_node_id,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                        }
                    ]
                },
                {
                    "id": 8,
                    "controllers": [],
                    "triggers": [
                        {
                            "kind": "temperature_value_trigger",
                            "next_node_id": self.head_next_node_id,
                            "source": "Water Temperature",
                            "operator": ">=",
                            "value": self.temperature + self.offset_temperature 
                        },
                        {
                            "kind": "temperature_value_trigger",
                            "next_node_id": 5,
                            "source": "Water Temperature",
                            "operator": "<=",
                            "value": self.temperature - self.offset_temperature 
                        },
                        {
                            "kind": "timer_trigger",
                            "timer_reference_id": 5,
                            "next_node_id": self.head_next_node_id,
                            "operator": ">=",
                            "value": 5
                        },
                        {
                            "kind": "button_trigger",
                            "next_node_id": self.head_next_node_id,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                        }
                    ]
                } 
            ]        
        },
        {
            "name": "click encoder",
            "nodes": [
                {
                    "id": 13,
                    "controllers": [],
                    "triggers": [
                        {
                            "kind": "button_trigger",
                            "next_node_id": 5,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                        }
                    ]
                }
            ]
        },
        {
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
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                        }
                    ]
                }
            ]
        },
        {
            "name": "retracting",
            "nodes": [
                {
                "id": 18,
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
                            "id": 2
                            }
                        }
                    },
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
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 23,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
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
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 23,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
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
                    },
                    {
                        "kind": "button_trigger",
                        "next_node_id": 23,
                        "gesture": "Single Tap",
                        "source": "Encoder Button"
                    }
                ]
                }
            ] 
        },
        {
            "name": "closing valve",
            "nodes": [
                {
                "id": 23,
                "controllers": [
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
                        "next_node_id": self.end_node_head,
                        "source": "Pressure Raw",
                        "operator": ">=",
                        "value": 0.2
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "next_node_id": -2,
                        "source": "Piston Position Raw",
                        "operator": ">=",
                        "value": self.max_piston_position 
                    }
                ]
                }
            ]
        }   
        ]
        
        return self.stages_head
    
    def tail_template(self):
        if self.click_to_purge:
            self.tail_next_node_id = 30
        else:
            self.tail_next_node_id = 48
        self.stages_tail = [
            {
              "name": "retracting",
              "nodes": [
                    {
                        "id": self.init_node_tail,
                        "controllers": [
                            {
                            "kind": "position_reference",
                            "id": 3
                            },
                            {
                            "kind": "weight_reference",
                            "id": 4
                            },
                            {
                            "kind": "time_reference",
                            "id": 8
                            }
                        ],
                        "triggers": [
                            {
                            "kind": "exit",
                            "next_node_id": 24
                            }
                        ]
                    },
                    {
                        "id": 27,
                        "controllers": [
                            {
                            "kind": "move_piston_controller",
                            "speed": 6,
                            "direction": "UP",
                            "algorithm": "Piston Fast"
                            }
                        ],
                        "triggers": [
                            {
                            "kind": "piston_speed_trigger",
                            "next_node_id": self.tail_next_node_id ,
                            "operator": "==",
                            "value": 0
                            },
                            {
                            "kind": "button_trigger",
                            "next_node_id": self.tail_next_node_id,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                            }
                        ]
                    },
                    {
                        "id": 24,
                        "controllers": [
                            {
                            "kind": "move_piston_controller",
                            "speed": 4,
                            "direction": "UP",
                            "algorithm": "Piston Fast"
                            }
                        ],
                        "triggers": [
                            {
                            "kind": "piston_position_trigger",
                            "next_node_id": 27,
                            "source": "Piston Position Raw",
                            "position_reference_id": 3,
                            "operator": "<=",
                            "value": -4
                            },
                            {
                            "kind": "button_trigger",
                            "next_node_id": self.tail_next_node_id,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                            }
                        ]
                    }
                ]
              },
            {
              "name": "click to purge",
              "nodes": [
                    {
                        "id": 30,
                        "controllers": [
                            {
                            "kind": "log_controller",
                            "message": "Click to purge"
                            }
                        ],
                        "triggers": [
                            {
                            "kind": "button_trigger",
                            "next_node_id": 31,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                            }
                        ]
                    }
                ]
              },
            {
              "name": "remove cup",
              "nodes": [
                  {
                      "id": 48,
                      "controllers": [
                          {
                          "kind": "time_reference",
                          "id": 15
                          }
                      ],
                      "triggers": [
                          {
                          "kind": "weight_value_trigger",
                          "weight_reference_id": 4,
                          "next_node_id": 49,
                          "source": "Weight Raw",
                          "operator": "<=",
                          "value": -5
                          },
                          {
                          "kind": "button_trigger",
                          "source": "Encoder Button",
                          "gesture": "Single Tap",
                          "next_node_id": 31
                          }
                      ]
                  },
                  {
                      "id": 49,
                      "controllers": [],
                      "triggers": [
                          {
                          "kind": "timer_trigger",
                          "timer_reference_id": 15,
                          "next_node_id": 31,
                          "operator": ">=",
                          "value": 5
                          },
                          {
                          "kind": "button_trigger",
                          "source": "Encoder Button",
                          "gesture": "Single Tap",
                          "next_node_id": 31
                          }
                      ]
                  }
              ]
            },
            {
              "name": "purge",
              "nodes": [
                    {
                        "id": 31,
                        "controllers": [
                            {
                            "kind": "move_piston_controller",
                            "speed": 6,
                            "direction": "DOWN",
                            "algorithm": "Piston Ease-In"
                            },
                            {
                            "kind": "time_reference",
                            "id": 8
                            }
                        ],
                        "triggers": [
                            {
                            "kind": "pressure_value_trigger",
                            "next_node_id": 32,
                            "source": "Pressure Raw",
                            "operator": ">=",
                            "value": 6
                            },
                            {
                            "kind": "piston_position_trigger",
                            "position_reference_id": 0,
                            "next_node_id": -2,
                            "source": "Piston Position Raw",
                            "operator": ">=",
                            "value": self.max_piston_position 
                            },
                            {
                            "kind": "button_trigger",
                            "next_node_id": -2,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                            }
                        ]
                    },
                    {
                        "id": 32,
                        "controllers": [
                            {
                            "kind": "pressure_controller",
                            "algorithm": "Pressure PID v1.0",
                            "curve": {
                                "id": 5,
                                "interpolation_kind": "linear_interpolation",
                                "points": [
                                    [
                                        0,
                                        6
                                    ]
                                ],
                                "reference": {
                                    "kind": "time",
                                    "id": 8
                                }
                            }
                            }
                        ],
                        "triggers": [
                            {
                            "kind": "piston_position_trigger",
                            "position_reference_id": 0,
                            "next_node_id": -2,
                            "source": "Piston Position Raw",
                            "operator": ">=",
                            "value": self.max_piston_position 
                            },
                            {
                            "kind": "button_trigger",
                            "next_node_id": -2,
                            "gesture": "Single Tap",
                            "source": "Encoder Button"
                            }
                        ]
                    }
                ]
              },
            {
              "name": "END_STAGE",
              "nodes": [
                    {
                        "id": -2,
                        "controllers": [
                            {
                            "kind": "end_profile"
                            }
                        ],
                        "triggers": []
                    }
                ]
              }          
        ]
        return self.stages_tail
     
     
    def complex_stages(self):
    
        return self.complex.to_complex(self.end_node_head, self.init_node_tail)
    
    def get_profile(self):
            self.complex_stage_build = self.head_template() + self.complex_stages() + self.tail_template()
            self.name_profile = self.complex.get_name() if self.complex.get_name() is not None else "Profile"
            
            self.profile_complex = {
                "name": self.name_profile,
                "stages": self.complex_stage_build
            }
            
            return self.profile_complex
        
        
        
if __name__ == '__main__':
    
    file_path = "simplified_json_example.json"
    with open(file_path, 'r') as file:
        data = json.load(file)
        
    sample = ComplexProfileConverter(False, True, 1000, 7000, data)
    
    # head_template = sample.head_template()
    # print(json.dumps(head_template, indent = 2))
    
    # tail_template = sample.tail_template()
    # print(json.dumps(tail_template, indent = 2))
    
    # complex_stages = sample.complex_stages()
    # print(json.dumps(complex_stages, indent = 2))
    
    profile = sample.get_profile()
    print(json.dumps(profile, indent = 2))
        
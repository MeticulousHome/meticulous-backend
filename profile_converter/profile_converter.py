import json
from .profile_json import *  # noqa: F401, F403
from .simplified_json import SimplifiedJson
from config import (
    MeticulousConfig,
    CONFIG_USER,
    MACHINE_ALLOW_STAGE_SKIPPING,
    PROFILE_PARTIAL_RETRACTION,
)


class ComplexProfileConverter:

    def __init__(
        self,
        click_to_start: bool,
        click_to_purge: bool,
        end_node_head: int,
        init_node_tail: int,
        parameters: dict = None,
    ):

        self.data = None
        self.parameters = parameters if parameters is not None else {}
        self.click_to_purge = click_to_purge if click_to_purge is not None else True
        self.end_node_head = end_node_head
        self.init_node_tail = init_node_tail
        self.complex = SimplifiedJson(self.parameters)
        self.temperature = self.complex.get_temperature()
        # Use this value to prevent overshooting with a global offset
        self.offset_temperature = 2
        self.max_piston_position = 75

    def head_template(self):
        no_skipping = not MeticulousConfig[CONFIG_USER][MACHINE_ALLOW_STAGE_SKIPPING]
        self.head_next_node_id = 16
        self.stages_head = [
            {
                "name": "prepare",
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
                                "value": self.max_piston_position - 2,
                            },
                            {
                                "kind": "piston_position_trigger",
                                "position_reference_id": 0,
                                "next_node_id": 11,
                                "source": "Piston Position Raw",
                                "operator": "<=",
                                "value": self.max_piston_position - 2,
                            },
                        ],
                    },
                ],
            },
            {
                "name": "purge",
                "nodes": [
                    {
                        "id": 11,
                        "controllers": [
                            {"kind": "time_reference", "id": 20},
                            {
                                "kind": "move_piston_controller",
                                "algorithm": "Piston Fast",
                                "direction": "DOWN",
                                "speed": 6,
                            },
                        ],
                        "triggers": (
                            [{"kind": "exit", "next_node_id": 1}]
                            if no_skipping
                            else [
                                {"kind": "exit", "next_node_id": 1},
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 45,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 1,
                        "controllers": [],
                        "triggers": (
                            [
                                {
                                    "kind": "pressure_value_trigger",
                                    "next_node_id": 2,
                                    "source": "Pressure Raw",
                                    "operator": ">=",
                                    "value": 6,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 20,
                                    "operator": ">=",
                                    "value": 1.5,
                                    "next_node_id": 40,
                                },
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "pressure_value_trigger",
                                    "next_node_id": 2,
                                    "source": "Pressure Raw",
                                    "operator": ">=",
                                    "value": 6,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 20,
                                    "operator": ">=",
                                    "value": 1.5,
                                    "next_node_id": 40,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 45,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 2,
                        "controllers": [
                            {
                                "kind": "pressure_controller",
                                "algorithm": "Pressure PID v1.0",
                                "curve": {
                                    "id": 1,
                                    "interpolation_kind": "linear_interpolation",
                                    "points": [[0, 6]],
                                    "reference": {"kind": "time", "id": 20},
                                },
                            }
                        ],
                        "triggers": (
                            [
                                {
                                    "kind": "piston_speed_trigger",
                                    "operator": "==",
                                    "value": 0,
                                    "next_node_id": 3,
                                }
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "piston_speed_trigger",
                                    "operator": "==",
                                    "value": 0,
                                    "next_node_id": 3,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 45,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 40,
                        "controllers": [
                            {
                                "kind": "move_piston_controller",
                                "algorithm": "Piston Ease-In",
                                "direction": "DOWN",
                                "speed": 6,
                            }
                        ],
                        "triggers": (
                            [
                                {
                                    "kind": "piston_speed_trigger",
                                    "operator": "==",
                                    "value": 0,
                                    "next_node_id": 3,
                                },
                                {
                                    "kind": "pressure_value_trigger",
                                    "source": "Pressure Raw",
                                    "operator": ">=",
                                    "value": 6,
                                    "next_node_id": 2,
                                },
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "piston_speed_trigger",
                                    "operator": "==",
                                    "value": 0,
                                    "next_node_id": 3,
                                },
                                {
                                    "kind": "pressure_value_trigger",
                                    "source": "Pressure Raw",
                                    "operator": ">=",
                                    "value": 6,
                                    "next_node_id": 2,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 45,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 3,
                        "controllers": [{"kind": "time_reference", "id": 21}],
                        "triggers": [{"kind": "exit", "next_node_id": 6}],
                    },
                    {
                        "id": 6,
                        "controllers": [],
                        "triggers": (
                            [
                                {
                                    "kind": "piston_speed_trigger",
                                    "operator": "!=",
                                    "value": 0,
                                    "next_node_id": 3,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 21,
                                    "operator": ">=",
                                    "value": 1,
                                    "next_node_id": 4,
                                },
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "piston_speed_trigger",
                                    "operator": "!=",
                                    "value": 0,
                                    "next_node_id": 3,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 21,
                                    "operator": ">=",
                                    "value": 1,
                                    "next_node_id": 4,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 45,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 4,
                        "controllers": [],
                        "triggers": (
                            [
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 20,
                                    "operator": ">=",
                                    "value": 1,
                                    "next_node_id": 45,
                                }
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 20,
                                    "operator": ">=",
                                    "value": 1,
                                    "next_node_id": 45,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 45,
                                },
                            ]
                        ),
                    },
                ],
            },
            {
                "name": "water detection",
                "nodes": [
                    {
                        "id": 45,
                        "controllers": [{"kind": "time_reference", "id": 12}],
                        "triggers": [{"kind": "exit", "next_node_id": 9}],
                    },
                    {
                        "id": 9,
                        "controllers": [{"kind": "time_reference", "id": 2}],
                        "triggers": [
                            {
                                "kind": "water_detection_trigger",
                                "next_node_id": 15,
                                "value": True,
                            },
                            {
                                "kind": "water_detection_trigger",
                                "next_node_id": 12,
                                "value": False,
                            },
                            {
                                "kind": "user_message_trigger",
                                "next_node_id": 15,
                            },
                        ],
                    },
                    {
                        "id": 12,
                        "controllers": [],
                        "triggers": (
                            [
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 2,
                                    "next_node_id": 9,
                                    "operator": ">=",
                                    "value": 2,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 12,
                                    "next_node_id": -2,
                                    "operator": ">=",
                                    "value": 300,
                                },
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 2,
                                    "next_node_id": 9,
                                    "operator": ">=",
                                    "value": 2,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 12,
                                    "next_node_id": -2,
                                    "operator": ">=",
                                    "value": 300,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 45,
                                },
                            ]
                        ),
                    },
                ],
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
                                    "points": [[0, self.temperature]],
                                    "reference": {"kind": "time", "id": 2},
                                },
                            },
                            {"kind": "position_reference", "id": 1},
                        ],
                        "triggers": (
                            [
                                {
                                    "kind": "temperature_value_trigger",
                                    "next_node_id": 5,
                                    "source": "Water Temperature",
                                    "operator": ">=",
                                    "value": self.temperature - self.offset_temperature,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 2,
                                    "next_node_id": -2,
                                    "operator": ">=",
                                    "value": 900,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 17,
                                },
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "temperature_value_trigger",
                                    "next_node_id": 5,
                                    "source": "Water Temperature",
                                    "operator": ">=",
                                    "value": self.temperature - self.offset_temperature,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 2,
                                    "next_node_id": -2,
                                    "operator": ">=",
                                    "value": 900,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 16,
                                },
                            ]
                        ),
                    }
                ],
            },
            {
                "name": "heating",
                "nodes": [
                    {
                        "id": 5,
                        "controllers": [{"kind": "time_reference", "id": 10}],
                        "triggers": (
                            [
                                {"kind": "exit", "next_node_id": 7},
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 17,
                                },
                            ]
                            if no_skipping
                            else [
                                {"kind": "exit", "next_node_id": 7},
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": self.head_next_node_id,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 7,
                        "controllers": [{"kind": "time_reference", "id": 5}],
                        "triggers": [
                            {
                                "kind": "timer_trigger",
                                "timer_reference_id": 10,
                                "next_node_id": 8,
                                "operator": ">=",
                                "value": 1,
                            }
                        ],
                    },
                    {
                        "id": 8,
                        "controllers": [],
                        "triggers": (
                            [
                                {
                                    "kind": "temperature_value_trigger",
                                    "next_node_id": self.head_next_node_id,
                                    "source": "Water Temperature",
                                    "operator": ">=",
                                    "value": self.temperature + self.offset_temperature,
                                },
                                {
                                    "kind": "temperature_value_trigger",
                                    "next_node_id": 5,
                                    "source": "Water Temperature",
                                    "operator": "<=",
                                    "value": self.temperature - self.offset_temperature,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 5,
                                    "next_node_id": self.head_next_node_id,
                                    "operator": ">=",
                                    "value": 5,
                                },
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "temperature_value_trigger",
                                    "next_node_id": self.head_next_node_id,
                                    "source": "Water Temperature",
                                    "operator": ">=",
                                    "value": self.temperature + self.offset_temperature,
                                },
                                {
                                    "kind": "temperature_value_trigger",
                                    "next_node_id": 5,
                                    "source": "Water Temperature",
                                    "operator": "<=",
                                    "value": self.temperature - self.offset_temperature,
                                },
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 5,
                                    "next_node_id": self.head_next_node_id,
                                    "operator": ">=",
                                    "value": 5,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": self.head_next_node_id,
                                },
                            ]
                        ),
                    },
                ],
            },
            {
                "name": "click to start",
                "nodes": [
                    {
                        "id": 16,
                        "controllers": [
                            {"kind": "time_reference", "id": 30},
                        ],
                        "triggers": [{"kind": "exit", "next_node_id": 25}],
                    },
                    {
                        "id": 25,
                        "controllers": [],
                        "triggers": [
                            {
                                "kind": "user_message_trigger",
                                "next_node_id": 17,
                            },
                            {
                                "kind": "timer_trigger",
                                "timer_reference_id": 30,
                                "next_node_id": -2,
                                "operator": ">=",
                                "value": 600,
                            },
                        ],
                    },
                ],
            },
            {
                "name": "retracting",
                "nodes": [
                    {
                        "id": 17,
                        "controllers": [
                            {
                                "kind": "temperature_controller",
                                "algorithm": "Cylinder Temperature PID v1.0",
                                "curve": {
                                    "id": 6,
                                    "interpolation_kind": "linear_interpolation",
                                    "points": [[0, 25]],
                                    "reference": {"kind": "time", "id": 2},
                                },
                            },
                        ],
                        "triggers": [{"kind": "exit", "next_node_id": 18}],
                    },
                    {
                        "id": 18,
                        "controllers": [
                            {
                                "kind": "piston_power_controller",
                                "algorithm": "Spring v1.0",
                                "curve": {
                                    "id": 7,
                                    "interpolation_kind": "linear_interpolation",
                                    "points": [[0, -100]],
                                    "reference": {"kind": "time", "id": 2},
                                },
                            },
                        ],
                        "triggers": (
                            [
                                {
                                    "kind": "piston_position_trigger",
                                    "next_node_id": 21,
                                    "source": "Piston Position Raw",
                                    "position_reference_id": 1,
                                    "operator": "<=",
                                    "value": -MeticulousConfig[CONFIG_USER][
                                        PROFILE_PARTIAL_RETRACTION
                                    ],
                                }
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "piston_position_trigger",
                                    "next_node_id": 21,
                                    "source": "Piston Position Raw",
                                    "position_reference_id": 1,
                                    "operator": "<=",
                                    "value": -MeticulousConfig[CONFIG_USER][
                                        PROFILE_PARTIAL_RETRACTION
                                    ],
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 21,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 21,
                        "controllers": [
                            {
                                "kind": "piston_power_controller",
                                "algorithm": "Spring v1.0",
                                "curve": {
                                    "id": 8,
                                    "interpolation_kind": "linear_interpolation",
                                    "points": [[0, 0]],
                                    "reference": {"kind": "time", "id": 2},
                                },
                            },
                            {"kind": "tare_controller"},
                            {"kind": "time_reference", "id": 4},
                        ],
                        "triggers": [{"kind": "exit", "next_node_id": 22}],
                    },
                    {
                        "id": 22,
                        "controllers": [
                            {"kind": "weight_reference", "id": 1},
                            {"kind": "position_reference", "id": 4},
                        ],
                        "triggers": [
                            {
                                "kind": "timer_trigger",
                                "timer_reference_id": 4,
                                "next_node_id": 23,
                                "operator": ">=",
                                "value": 5,
                            }
                        ],
                    },
                ],
            },
            {
                "name": "closing valve",
                "nodes": [
                    {
                        "id": 23,
                        "controllers": [
                            {
                                "kind": "move_piston_controller",
                                "speed": 6,
                                "direction": "DOWN",
                                "algorithm": "Piston Ease-In",
                            },
                            {"kind": "time_reference", "id": 1},
                        ],
                        "triggers": (
                            [
                                {
                                    "kind": "piston_position_trigger",
                                    "position_reference_id": 4,
                                    "next_node_id": self.end_node_head,
                                    "source": "Piston Position Raw",
                                    "operator": ">=",
                                    "value": 4,
                                }
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "piston_position_trigger",
                                    "position_reference_id": 4,
                                    "next_node_id": self.end_node_head,
                                    "source": "Piston Position Raw",
                                    "operator": ">=",
                                    "value": 4,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": self.end_node_head,
                                },
                            ]
                        ),
                    }
                ],
            },
        ]

        return self.stages_head

    def tail_template(self):
        no_skipping = not MeticulousConfig[CONFIG_USER][MACHINE_ALLOW_STAGE_SKIPPING]

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
                            {"kind": "position_reference", "id": 3},
                            {"kind": "weight_reference", "id": 4},
                            {"kind": "time_reference", "id": 8},
                        ],
                        "triggers": [{"kind": "exit", "next_node_id": 24}],
                    },
                    {
                        "id": 24,
                        "controllers": [
                            {
                                "kind": "move_piston_controller",
                                "speed": 4,
                                "direction": "UP",
                                "algorithm": "Piston Fast",
                            }
                        ],
                        "triggers": [
                            {
                                "kind": "piston_position_trigger",
                                "next_node_id": 27,
                                "source": "Piston Position Raw",
                                "position_reference_id": 3,
                                "operator": "<=",
                                "value": -4,
                            }
                        ],
                    },
                    {
                        "id": 27,
                        "controllers": [
                            {
                                "kind": "piston_power_controller",
                                "algorithm": "Spring v1.0",
                                "curve": {
                                    "id": 9,
                                    "interpolation_kind": "linear_interpolation",
                                    "points": [[0, -100]],
                                    "reference": {"kind": "time", "id": 8},
                                },
                            },
                        ],
                        "triggers": [
                            {
                                "kind": "piston_speed_trigger",
                                "next_node_id": self.tail_next_node_id,
                                "operator": "==",
                                "value": 0,
                            }
                        ],
                    },
                ],
            },
            {
                "name": "click to purge",
                "nodes": [
                    {
                        "id": 30,
                        "controllers": [],
                        "triggers": [
                            {
                                "kind": "user_message_trigger",
                                "next_node_id": 31,
                            }
                        ],
                    }
                ],
            },
            {
                "name": "remove cup",
                "nodes": [
                    {
                        "id": 48,
                        "controllers": [{"kind": "time_reference", "id": 15}],
                        "triggers": (
                            [
                                {
                                    "kind": "weight_value_trigger",
                                    "weight_reference_id": 4,
                                    "next_node_id": 49,
                                    "source": "Weight Raw",
                                    "operator": "<=",
                                    "value": -5,
                                }
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "weight_value_trigger",
                                    "weight_reference_id": 4,
                                    "next_node_id": 49,
                                    "source": "Weight Raw",
                                    "operator": "<=",
                                    "value": -5,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 31,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 49,
                        "controllers": [],
                        "triggers": (
                            [
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 15,
                                    "next_node_id": 31,
                                    "operator": ">=",
                                    "value": 5,
                                }
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 15,
                                    "next_node_id": 31,
                                    "operator": ">=",
                                    "value": 5,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": 31,
                                },
                            ]
                        ),
                    },
                ],
            },
            {
                "name": "purge",
                "nodes": [
                    {
                        "id": 31,
                        "controllers": [
                            {"kind": "time_reference", "id": 22},
                        ],
                        "triggers": [
                            {"kind": "exit", "next_node_id": 32},
                        ],
                    },
                    {
                        "id": 32,
                        "controllers": [
                            {
                                "kind": "pressure_controller",
                                "algorithm": "Pressure PID v1.0",
                                "curve": {
                                    "id": 10,
                                    "interpolation_kind": "linear_interpolation",
                                    "points": [[0, 6]],
                                    "time_reference_id": 22,
                                },
                            }
                        ],
                        "triggers": (
                            [
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 22,
                                    "operator": ">=",
                                    "value": 2,
                                    "next_node_id": 34,
                                },
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "timer_trigger",
                                    "timer_reference_id": 22,
                                    "operator": ">=",
                                    "value": 2,
                                    "next_node_id": 34,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": -2,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 34,
                        "controllers": [],
                        "triggers": (
                            [
                                {
                                    "kind": "piston_speed_trigger",
                                    "operator": "==",
                                    "value": 0,
                                    "next_node_id": 35,
                                }
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "piston_speed_trigger",
                                    "operator": "==",
                                    "value": 0,
                                    "next_node_id": 35,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": -2,
                                },
                            ]
                        ),
                    },
                    {
                        "id": 35,
                        "controllers": [],
                        "triggers": (
                            [
                                {
                                    "kind": "pressure_value_trigger",
                                    "source": "Pressure Raw",
                                    "operator": "<=",
                                    "value": 0.5,
                                    "next_node_id": -2,
                                },
                            ]
                            if no_skipping
                            else [
                                {
                                    "kind": "pressure_value_trigger",
                                    "source": "Pressure Raw",
                                    "operator": "<=",
                                    "value": 0.5,
                                    "next_node_id": -2,
                                },
                                {
                                    "kind": "user_message_trigger",
                                    "next_node_id": -2,
                                },
                            ]
                        ),
                    },
                ],
            },
            {
                "name": "END_STAGE",
                "nodes": [
                    {"id": -2, "controllers": [{"kind": "end_profile"}], "triggers": []}
                ],
            },
        ]
        return self.stages_tail

    def complex_stages(self):

        return self.complex.to_complex(self.end_node_head, self.init_node_tail)

    def get_profile(self):
        self.complex_stage_build = (
            self.head_template() + self.complex_stages() + self.tail_template()
        )
        self.name_profile = (
            self.complex.get_name()
            if self.complex.get_name() is not None
            else "Profile"
        )

        self.profile_complex = {
            "name": self.name_profile,
            "stages": self.complex_stage_build,
        }

        return self.profile_complex


if __name__ == "__main__":

    file_path = "simplified_json_example.json"
    with open(file_path, "r") as file:
        data = json.load(file)

    sample = ComplexProfileConverter(False, True, 1000, 7000, data)

    # head_template = sample.head_template()
    # print(json.dumps(head_template, indent = 2))

    # tail_template = sample.tail_template()
    # print(json.dumps(tail_template, indent = 2))

    # complex_stages = sample.complex_stages()
    # print(json.dumps(complex_stages, indent = 2))

    profile = sample.get_profile()
    print(json.dumps(profile, indent=2))

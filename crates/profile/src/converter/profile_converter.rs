//! `profile_converter/profile_converter.py`: `ComplexProfileConverter` and its
//! head and tail templates.
//!
//! The templates are the Python's dict literals transcribed in order; with
//! stage skipping allowed most nodes gain a button trigger, and a few swap a
//! trigger for one, exactly where the Python's `if no_skipping else` lists
//! differ.

use serde_json::{Value, json};

use super::MAX_PISTON_POSITION;
use super::simplified_json::SimplifiedJson;
use crate::pynum::{PyNum, float_value};
use crate::pyops::PyExc;
use crate::pyrepr::type_name;

/// `ComplexProfileConverter(click_to_start, click_to_purge, end_node_head,
/// init_node_tail, parameters)`, with the two config values its templates
/// read (`MeticulousConfig[CONFIG_USER][...]`) passed in.
#[derive(Debug, Clone)]
pub struct ComplexProfileConverter<'p> {
    pub click_to_purge: bool,
    pub end_node_head: i64,
    pub init_node_tail: i64,
    pub complex: SimplifiedJson<'p>,
    temperature: &'p Value,
    /// "Use this value to prevent overshooting with a global offset."
    offset_temperature: i64,
    max_piston_position: i64,
    /// `MACHINE_ALLOW_STAGE_SKIPPING`.
    allow_stage_skipping: bool,
    /// `PROFILE_PARTIAL_RETRACTION`, always a float in the Python config.
    partial_retraction: f64,
}

impl<'p> ComplexProfileConverter<'p> {
    /// Fails like `self.complex.get_temperature()` when the document has no
    /// `temperature`.
    pub fn new(
        click_to_purge: bool,
        end_node_head: i64,
        init_node_tail: i64,
        parameters: &'p Value,
        allow_stage_skipping: bool,
        partial_retraction: f64,
    ) -> Result<Self, PyExc> {
        let complex = SimplifiedJson::new(parameters);
        let temperature = complex.get_temperature()?;
        Ok(ComplexProfileConverter {
            click_to_purge,
            end_node_head,
            init_node_tail,
            complex,
            temperature,
            offset_temperature: 2,
            max_piston_position: MAX_PISTON_POSITION,
            allow_stage_skipping,
            partial_retraction,
        })
    }

    /// `self.temperature - self.offset_temperature` and
    /// `self.temperature + self.offset_temperature`, keeping an int an int.
    fn offset_temperatures(&self) -> Result<(Value, Value), PyExc> {
        let temperature = match self.temperature {
            Value::Number(n) => PyNum::from_number(n),
            Value::Bool(b) => PyNum::Int(i128::from(*b)),
            other => {
                return Err(PyExc::Type(format!(
                    "unsupported operand type(s) for -: '{}' and 'int'",
                    type_name(other)
                )));
            }
        };
        let offset = PyNum::Int(i128::from(self.offset_temperature));
        Ok((
            (temperature - offset).to_value(),
            (temperature + offset).to_value(),
        ))
    }

    /// `head_template()`: prepare, purge, water detection, heating, click to
    /// start, retracting, closing valve.
    pub fn head_template(&self) -> Result<Vec<Value>, PyExc> {
        let no_skipping = !self.allow_stage_skipping;
        let head_next_node_id = 16;
        let (temperature_low, temperature_high) = self.offset_temperatures()?;
        let temperature = self.temperature.clone();
        let piston_limit = self.max_piston_position - 2;
        let retraction = float_value(-self.partial_retraction);
        let end_node_head = self.end_node_head;

        let button = |next: i64| {
            json!({
                "kind": "button_trigger",
                "next_node_id": next,
                "gesture": "Single Tap",
                "source": "Encoder Button",
            })
        };
        // The no-skipping triggers, plus a button to `next` when skipping.
        let skippable = |mut triggers: Vec<Value>, next: i64| {
            if !no_skipping {
                triggers.push(button(next));
            }
            Value::Array(triggers)
        };

        Ok(vec![
            json!({
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
                                "value": piston_limit,
                            },
                            {
                                "kind": "piston_position_trigger",
                                "position_reference_id": 0,
                                "next_node_id": 11,
                                "source": "Piston Position Raw",
                                "operator": "<=",
                                "value": piston_limit,
                            },
                        ],
                    },
                ],
            }),
            json!({
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
                        "triggers": skippable(vec![json!({"kind": "exit", "next_node_id": 1})], 45),
                    },
                    {
                        "id": 1,
                        "controllers": [],
                        "triggers": skippable(vec![
                            json!({
                                "kind": "pressure_value_trigger",
                                "next_node_id": 2,
                                "source": "Pressure Raw",
                                "operator": ">=",
                                "value": 6,
                            }),
                            json!({
                                "kind": "timer_trigger",
                                "timer_reference_id": 20,
                                "operator": ">=",
                                "value": 1.5,
                                "next_node_id": 40,
                            }),
                        ], 45),
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
                        "triggers": skippable(vec![json!({
                            "kind": "piston_speed_trigger",
                            "operator": "==",
                            "value": 0,
                            "next_node_id": 3,
                        })], 45),
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
                        "triggers": skippable(vec![
                            json!({
                                "kind": "piston_speed_trigger",
                                "operator": "==",
                                "value": 0,
                                "next_node_id": 3,
                            }),
                            json!({
                                "kind": "pressure_value_trigger",
                                "source": "Pressure Raw",
                                "operator": ">=",
                                "value": 6,
                                "next_node_id": 2,
                            }),
                        ], 45),
                    },
                    {
                        "id": 3,
                        "controllers": [{"kind": "time_reference", "id": 21}],
                        "triggers": [{"kind": "exit", "next_node_id": 6}],
                    },
                    {
                        "id": 6,
                        "controllers": [],
                        "triggers": skippable(vec![
                            json!({
                                "kind": "piston_speed_trigger",
                                "operator": "!=",
                                "value": 0,
                                "next_node_id": 3,
                            }),
                            json!({
                                "kind": "timer_trigger",
                                "timer_reference_id": 21,
                                "operator": ">=",
                                "value": 1,
                                "next_node_id": 4,
                            }),
                        ], 45),
                    },
                    {
                        "id": 4,
                        "controllers": [],
                        "triggers": skippable(vec![json!({
                            "kind": "timer_trigger",
                            "timer_reference_id": 20,
                            "operator": ">=",
                            "value": 1,
                            "next_node_id": 45,
                        })], 45),
                    },
                ],
            }),
            json!({
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
                                "value": true,
                            },
                            {
                                "kind": "water_detection_trigger",
                                "next_node_id": 12,
                                "value": false,
                            },
                            {
                                "kind": "button_trigger",
                                "next_node_id": 15,
                                "gesture": "Single Tap",
                                "source": "Encoder Button",
                            },
                        ],
                    },
                    {
                        "id": 12,
                        "controllers": [],
                        "triggers": skippable(vec![
                            json!({
                                "kind": "timer_trigger",
                                "timer_reference_id": 2,
                                "next_node_id": 9,
                                "operator": ">=",
                                "value": 2,
                            }),
                            json!({
                                "kind": "timer_trigger",
                                "timer_reference_id": 12,
                                "next_node_id": -2,
                                "operator": ">=",
                                "value": 300,
                            }),
                        ], 45),
                    },
                ],
            }),
            json!({
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
                                    "points": [[0, temperature]],
                                    "reference": {"kind": "time", "id": 2},
                                },
                            },
                            {"kind": "position_reference", "id": 1},
                        ],
                        "triggers": [
                            {
                                "kind": "temperature_value_trigger",
                                "next_node_id": 5,
                                "source": "Water Temperature",
                                "operator": ">=",
                                "value": temperature_low,
                            },
                            {
                                "kind": "timer_trigger",
                                "timer_reference_id": 2,
                                "next_node_id": -2,
                                "operator": ">=",
                                "value": 900,
                            },
                            if no_skipping {
                                json!({
                                    "kind": "user_message_trigger",
                                    "next_node_id": 17,
                                    "gesture": "Single Tap",
                                    "source": "Encoder Button",
                                })
                            } else {
                                button(16)
                            },
                        ],
                    }
                ],
            }),
            json!({
                "name": "heating",
                "nodes": [
                    {
                        "id": 5,
                        "controllers": [{"kind": "time_reference", "id": 10}],
                        "triggers": [
                            {"kind": "exit", "next_node_id": 7},
                            if no_skipping {
                                json!({
                                    "kind": "user_message_trigger",
                                    "next_node_id": 17,
                                    "gesture": "Single Tap",
                                    "source": "Encoder Button",
                                })
                            } else {
                                button(head_next_node_id)
                            },
                        ],
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
                        "triggers": skippable(vec![
                            json!({
                                "kind": "temperature_value_trigger",
                                "next_node_id": head_next_node_id,
                                "source": "Water Temperature",
                                "operator": ">=",
                                "value": temperature_high,
                            }),
                            json!({
                                "kind": "temperature_value_trigger",
                                "next_node_id": 5,
                                "source": "Water Temperature",
                                "operator": "<=",
                                "value": temperature_low,
                            }),
                            json!({
                                "kind": "timer_trigger",
                                "timer_reference_id": 5,
                                "next_node_id": head_next_node_id,
                                "operator": ">=",
                                "value": 5,
                            }),
                        ], head_next_node_id),
                    },
                ],
            }),
            json!({
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
                                "gesture": "Single Tap",
                                "source": "Encoder Button",
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
            }),
            json!({
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
                        "triggers": skippable(vec![json!({
                            "kind": "piston_position_trigger",
                            "next_node_id": 21,
                            "source": "Piston Position Raw",
                            "position_reference_id": 1,
                            "operator": "<=",
                            "value": retraction,
                        })], 21),
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
            }),
            json!({
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
                        "triggers": skippable(vec![json!({
                            "kind": "piston_position_trigger",
                            "position_reference_id": 4,
                            "next_node_id": end_node_head,
                            "source": "Piston Position Raw",
                            "operator": ">=",
                            "value": 4,
                        })], end_node_head),
                    }
                ],
            }),
        ])
    }

    /// `tail_template()`: retracting, click to purge, remove cup, purge,
    /// END_STAGE.
    pub fn tail_template(&self) -> Vec<Value> {
        let no_skipping = !self.allow_stage_skipping;
        let tail_next_node_id = if self.click_to_purge { 30 } else { 48 };

        // The tail's button triggers spell their keys in three orders.
        let button_source_first = |next: i64| {
            json!({
                "kind": "button_trigger",
                "source": "Encoder Button",
                "gesture": "Single Tap",
                "next_node_id": next,
            })
        };
        let button_next_first = |next: i64| {
            json!({
                "kind": "button_trigger",
                "next_node_id": next,
                "gesture": "Single Tap",
                "source": "Encoder Button",
            })
        };
        let button_gesture_first = |next: i64| {
            json!({
                "kind": "button_trigger",
                "gesture": "Single Tap",
                "source": "Encoder Button",
                "next_node_id": next,
            })
        };
        let skippable = |mut triggers: Vec<Value>, button: Value| {
            if !no_skipping {
                triggers.push(button);
            }
            Value::Array(triggers)
        };

        vec![
            json!({
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
                                "next_node_id": tail_next_node_id,
                                "operator": "==",
                                "value": 0,
                            }
                        ],
                    },
                ],
            }),
            json!({
                "name": "click to purge",
                "nodes": [
                    {
                        "id": 30,
                        "controllers": [],
                        "triggers": [
                            {
                                "kind": "user_message_trigger",
                                "next_node_id": 31,
                                "gesture": "Single Tap",
                                "source": "Encoder Button",
                            }
                        ],
                    }
                ],
            }),
            json!({
                "name": "remove cup",
                "nodes": [
                    {
                        "id": 48,
                        "controllers": [{"kind": "time_reference", "id": 15}],
                        "triggers": skippable(vec![json!({
                            "kind": "weight_value_trigger",
                            "weight_reference_id": 4,
                            "next_node_id": 49,
                            "source": "Weight Raw",
                            "operator": "<=",
                            "value": -5,
                        })], button_source_first(31)),
                    },
                    {
                        "id": 49,
                        "controllers": [],
                        "triggers": skippable(vec![json!({
                            "kind": "timer_trigger",
                            "timer_reference_id": 15,
                            "next_node_id": 31,
                            "operator": ">=",
                            "value": 5,
                        })], button_source_first(31)),
                    },
                ],
            }),
            json!({
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
                        "triggers": skippable(vec![json!({
                            "kind": "timer_trigger",
                            "timer_reference_id": 22,
                            "operator": ">=",
                            "value": 2,
                            "next_node_id": 34,
                        })], button_next_first(-2)),
                    },
                    {
                        "id": 34,
                        "controllers": [],
                        "triggers": skippable(vec![json!({
                            "kind": "piston_speed_trigger",
                            "operator": "==",
                            "value": 0,
                            "next_node_id": 35,
                        })], button_next_first(-2)),
                    },
                    {
                        "id": 35,
                        "controllers": [],
                        "triggers": skippable(vec![json!({
                            "kind": "pressure_value_trigger",
                            "source": "Pressure Raw",
                            "operator": "<=",
                            "value": 0.5,
                            "next_node_id": -2,
                        })], button_gesture_first(-2)),
                    },
                ],
            }),
            json!({
                "name": "END_STAGE",
                "nodes": [{"id": -2, "controllers": [{"kind": "end_profile"}], "triggers": []}],
            }),
        ]
    }
}

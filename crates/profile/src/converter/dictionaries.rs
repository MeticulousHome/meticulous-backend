//! The strings of `profile_converter/dictionaries.py` the reachable builders
//! use, named after the enum keys that select them.

/// `algorithms_type[AlgorithmType.*][*AlgorithmType.*]`.
pub mod algorithms_type {
    pub const PRESSURE_PID_V1: &str = "Pressure PID v1.0";
    pub const FLOW_PID_V1: &str = "Flow PID v1.0";
}

/// `reference_type[ReferenceType.CONTROL][ReferenceType.*]`.
pub mod reference_type {
    pub const TIME: &str = "time_reference";
    pub const POSITION: &str = "position_reference";
    pub const WEIGHT: &str = "weight_reference";
}

/// `trigger_type[TriggerType.*]`.
pub mod trigger_type {
    pub const PISTON_POSITION: &str = "piston_position_trigger";
    pub const WEIGHT: &str = "weight_value_trigger";
    pub const BUTTON: &str = "button_trigger";
    pub const EXIT: &str = "exit";
}

/// `source_type[SourceType.*][...]`.
pub mod source_type {
    pub const RAW_FLOW: &str = "Flow Raw";
    pub const RAW_PRESSURE: &str = "Pressure Raw";
    pub const PREDICTIVE_WEIGHT: &str = "Weight Predictive";
    pub const BUTTON_ENCODER_BUTTON: &str = "Encoder Button";
    pub const GESTURE_SINGLE: &str = "Single Tap";
    pub const GESTURE_LONG: &str = "Long Press";
}

/// `operator_type[TriggerOperatorType.*]`.
pub mod operator_type {
    pub const GREATER_THAN_OR_EQUAL: &str = ">=";
}

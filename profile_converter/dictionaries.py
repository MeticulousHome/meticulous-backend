from .enums import *

controllers_type = {
    ControllerType.POWER: "piston_power_controller",
    ControllerType.FLOW: "flow_controller",
    ControllerType.PRESSURE: "pressure_controller",
    ControllerType.WEIGHT: "weight_controller",
    ControllerType.SPEED: "move_piston_controller",
    ControllerType.TEMPERATURE: "temperature_controller",
    ControllerType.TARE: "tare_controller",
    ControllerType.MESSAGE: "log_controller",
    ControllerType.END: "end_profile",
}

algorithms_type = {
    AlgorithmType.PRESSURE: {
        PressureAlgorithmType.PID_V1: "Pressure PID v1.0",
        PressureAlgorithmType.PID_V2: "Pressure PID v2.0",
    },
    AlgorithmType.POWER: {PowerAlgorithmType.SPRING: "Spring v1.0"},
    AlgorithmType.TEMPERATURE: {
        TemperatureAlgorithmType.WATER: "Water Temperature PID v1.0",
        TemperatureAlgorithmType.CYLINDER: "Cylinder Temperature PID v1.0",
        TemperatureAlgorithmType.TUBE: "Tube Temperature PID v1.0",
        TemperatureAlgorithmType.PLUNGER: "Plunger Temperature PID v1.0",
        TemperatureAlgorithmType.STABLE: "Stable Temperature",
    },
    AlgorithmType.FLOW: {FlowAlgorithmType.PID_V1: "Flow PID v1.0"},
    AlgorithmType.WEIGHT: {WeightAlgorithmType.PID_V1: "Weight PID v1.0"},
    AlgorithmType.SPEED: {
        SpeedAlgorithmType.EASE_IN: "Piston Ease-In",
        SpeedAlgorithmType.FAST: "Piston Fast",
    },
}

reference_type = {
    ReferenceType.CURVE: {
        ReferenceType.TIME: "time",
        ReferenceType.POSITION: "position",
        ReferenceType.WEIGHT: "weight",
    },
    ReferenceType.CONTROL: {
        ReferenceType.TIME: "time_reference",
        ReferenceType.POSITION: "position_reference",
        ReferenceType.WEIGHT: "weight_reference",
    },
}

curve_interpolation = {
    CurveInterpolationType.LINEAR: "linear_interpolation",
    CurveInterpolationType.CATMULL: "catmull_interpolation",
}

messages = {
    MessageType.NO_WATER: "No Water",
    MessageType.REMOVE_CUP: "Remove Cup",
    MessageType.PURGE: "Purge",
    MessageType.START_CLICK: "Click to start",
    MessageType.PURGE_CLICK: "Click to purge",
}

directions = {DirectionType.FORWARD: "DOWN", DirectionType.BACKWARD: "UP"}

trigger_type = {
    TriggerType.PISTON_POSITION: "piston_position_trigger",
    TriggerType.SPEED: "piston_speed_trigger",
    TriggerType.TIME: "timer_trigger",
    TriggerType.WEIGHT: "weight_value_trigger",
    TriggerType.BUTTON: "button_trigger",
    TriggerType.WATER_DETECTION: "water_detection_trigger",
    TriggerType.CURVE: {
        TriggerType.FLOW: "flow_curve_trigger",
        TriggerType.PRESSURE: "pressure_curve_trigger",
        TriggerType.TEMPERATURE: "temperature_curve_trigger",
        TriggerType.POWER: "piston_power_curve_trigger",
    },
    TriggerType.VALUE: {
        TriggerType.FLOW: "flow_value_trigger",
        TriggerType.PRESSURE: "pressure_value_trigger",
        TriggerType.TEMPERATURE: "temperature_value_trigger",
        TriggerType.POWER: "piston_power_value_trigger",
    },
    TriggerType.EXIT: "exit",
    TriggerType.USER_MESSAGE: "user_message_trigger",
}

source_type = {
    SourceType.RAW: {
        SourceType.FLOW: "Flow Raw",
        SourceType.PRESSURE: "Pressure Raw",
        SourceType.WEIGHT: "Weight Raw",
        SourceType.POWER: "Raw Piston Power",
    },
    SourceType.AVERAGE: {
        SourceType.FLOW: "Flow Average",
        SourceType.PRESSURE: "Pressure Average",
        SourceType.WEIGHT: "Weight Average",
        SourceType.POWER: "Average Piston Power",
    },
    SourceType.PREDICTIVE: {
        SourceType.FLOW: "Flow Predictive",
        SourceType.PRESSURE: "Pressure Predictive",
        SourceType.WEIGHT: "Weight Predictive",
        SourceType.POWER: "Predictive Piston Power",
    },
    SourceType.TEMPERATURE: {
        TemperatureSourceType.TUBE: "Tube Temperature",
        TemperatureSourceType.CYLINDER: "Cylinder Temperature",
        TemperatureSourceType.PLUNGER: "Plunger Temperature",
        TemperatureSourceType.WATER: "Water Temperature",
        TemperatureSourceType.CYLINDER_AVERAGE: "Cylinder Temperature Average",
    },
    SourceType.BUTTON: {
        ButtonSourceType.START: "Start Button",
        ButtonSourceType.TARE: "Tare Button",
        ButtonSourceType.ENCODER: "Encoder",
        ButtonSourceType.ENCODER_BUTTON: "Encoder Button",
    },
    SourceType.GESTURE: {
        ButtonGestureSourceType.SINGLE: "Single Tap",
        ButtonGestureSourceType.DOUBLE: "Double Tap",
        ButtonGestureSourceType.RIGHT: "Right",
        ButtonGestureSourceType.LEFT: "Left",
        ButtonGestureSourceType.PRESSED: "Pressed",
        ButtonGestureSourceType.RELEASED: "Released",
        ButtonGestureSourceType.LONG: "Long Press",
    },
}

operator_type = {
    TriggerOperatorType.GREATER_THAN: ">",
    TriggerOperatorType.LESS_THAN: "<",
    TriggerOperatorType.EQUAL: "==",
    TriggerOperatorType.GREATER_THAN_OR_EQUAL: ">=",
    TriggerOperatorType.LESS_THAN_OR_EQUAL: "<=",
}

import enums

controllers_type = {
    enums.ControllerType.POWER: "piston_power_controller",
    enums.ControllerType.FLOW : "flow_controller",
    enums.ControllerType.PRESSURE : "pressure_controller",
    enums.ControllerType.WEIGHT : "weight_controller",
    enums.ControllerType.SPEED: "move_piston_controller",
    enums.ControllerType.TEMPERATURE : "temperature_controller",
    enums.ControllerType.TARE : "tare_controller",
    enums.ControllerType.MESSAGE : "log_controller",
    enums.ControllerType.END : "end_profile",
}

algorithms_type = {
    enums.AlgorithmType.PRESSURE : {
        enums.Pressure_Algorithm_Type.PID_V1 : "Pressure PID v1.0",
        enums.Pressure_Algorithm_Type.PID_V2 : "Pressure PID v2.0",
    },
    enums.AlgorithmType.POWER: {
        enums.Power_Algorithm_Type.SPRING :  "Spring v1.0"        
    },
    enums.AlgorithmType.TEMPERATURE : {
        enums.Temperature_Algorithm_Type.WATER : "Water Temperature PID v1.0",
        enums.Temperature_Algorithm_Type.CYLINDER : "Cylinder Temperature PID v1.0",
        enums.Temperature_Algorithm_Type.TUBE: "Tube Temperature PID v1.0",
        enums.Temperature_Algorithm_Type.PLUNGER : "Plunger Temperature PID v1.0",
        enums.Temperature_Algorithm_Type.STABLE : "Stable Temperature"
    },
    enums.AlgorithmType.FLOW : {
      enums.Flow_Algorithm_Type.PID_V1 : "Flow PID v1.0"  
    },
    enums.AlgorithmType.WEIGHT : {
        enums.Weight_Algorithm_Type.PID_V1 : "Weight PID v1.0"    
    },
    enums.AlgorithmType.SPEED : {
        enums.Speed_Algorithm_Type.EASE_IN : "Piston Ease-In",
        enums.Speed_Algorithm_Type.FAST : "Piston Fast",
    }
} 

reference_type = {
    enums.ReferenceType.CURVE : {
        enums.ReferenceType.TIME : "time",
        enums.ReferenceType.POSITION : "position",
        enums.ReferenceType.WEIGHT : "weight"
    },
    enums.ReferenceType.CONTROL : {
        enums.ReferenceType.TIME : "time_reference",
        enums.ReferenceType.POSITION : "position_reference",
        enums.ReferenceType.WEIGHT : "weight_reference"
    }
}

curve_interpolation = {
    enums.CurveInterpolationType.LINEAR: "linear_interpolation",
    enums.CurveInterpolationType.CATMULL : "catmull_interpolation"
}
        
messages = {
    enums.Message_Type.NO_WATER : "No Water",
    enums.Message_Type.REMOVE_CUP : "Remove Cup",
    enums.Message_Type.PURGE : "Purge",
    enums.Message_Type.START_CLICK : "Click to start",
    enums.Message_Type.PURGE_CLICK : "Click to purge"
}

directions = {
    enums.Direction_Type.FORWARD : "DOWN",
    enums.Direction_Type.BACKWARD : "UP"
}

trigger_type = {
    enums.TriggerType.PISTON_POSITION : "piston_position_trigger",
    enums.TriggerType.SPEED: "speed_trigger",
    enums.TriggerType.TIME : "time_trigger",
    enums.TriggerType.WEIGHT : "weight_trigger",
    enums.TriggerType.BUTTON : "button_trigger",
    enums.TriggerType.WATER_DETECTION : "water_detection_trigger",
    enums.TriggerType.CURVE:{
        enums.TriggerType.FLOW : "flow_curve_trigger",
        enums.TriggerType.PRESSURE : "pressure_curve_trigger",
        enums.TriggerType.TEMPERATURE : "temperature_curve_trigger",
        enums.TriggerType.POWER : "power_curve_trigger",
    },
    enums.TriggerType.VALUE :{
        enums.TriggerType.FLOW :"flow_value_trigger",
        enums.TriggerType.PRESSURE : "pressure_value_trigger",
        enums.TriggerType.TEMPERATURE: "temperature_value_trigger",
        enums.TriggerType.POWER :  "power_value_trigger",
    },
    enums.TriggerType.EXIT : "exit"
}

source_type = {
    enums.SourceType.RAW : {
        enums.SourceType.FLOW : "Flow Raw",
        enums.SourceType.PRESSURE : "Pressure Raw",
        enums.SourceType.WEIGHT : "Weight Raw",
        enums.SourceType.POWER : "Piston Power Raw"
    },
    enums.SourceType.AVERAGE : {
        enums.SourceType.FLOW : "Flow Average",
        enums.SourceType.PRESSURE : "Pressure Average",
        enums.SourceType.WEIGHT : "Weight Average",
        enums.SourceType.POWER : "Piston Power Average"
    },
    enums.SourceType.PREDICTIVE : {
        enums.SourceType.FLOW : "Flow Predictive",
        enums.SourceType.PRESSURE : "Pressure Predictive",
        enums.SourceType.WEIGHT  : "Weight Predictive",
        enums.SourceType.POWER : "Piston Power Predictive"
    },
    enums.SourceType.TEMPERATURE: {
        enums.TemperatureSourceType.TUBE:"Tube Temperature", 
        enums.TemperatureSourceType.CYLINDER:"Cylinder Temperature", 
        enums.TemperatureSourceType.PLUNGER:"Plunger Temperature", 
        enums.TemperatureSourceType.WATER:"Water Temperature", 
        enums.TemperatureSourceType.CYLINDER_AVERAGE: "Cylinder Temperature Average"
    },
    enums.SourceType.BUTTON: {
        enums.ButtonSourceType.START : "Start Button",
        enums.ButtonSourceType.TARE : "Tare Button",
        enums.ButtonSourceType.ENCODER : "Encoder",
        enums.ButtonSourceType.ENCODER_BUTTON : "Encoder Button"
        },
    
    enums.SourceType.GESTURE:{ 
        enums.ButtonGestureSourceType.SINGLE: "Single Tap",
        enums.ButtonGestureSourceType.DOUBLE: "Double Tap",
        enums.ButtonGestureSourceType.RIGHT: "Right",
        enums.ButtonGestureSourceType.LEFT: "Left",
        enums.ButtonGestureSourceType.PRESSED: "Pressed",
        enums.ButtonGestureSourceType.RELEASED: "Released",
        enums.ButtonGestureSourceType.LONG: "Long Press"  
    }
}

operator_type = {
    enums.TriggerOperatorType.GREATER_THAN : ">",
    enums.TriggerOperatorType.LESS_THAN: "<",
    enums.TriggerOperatorType.EQUAL : "==",
    enums.TriggerOperatorType.GREATER_THAN_OR_EQUAL: ">=",
    enums.TriggerOperatorType.LESS_THAN_OR_EQUAL: "<="
}
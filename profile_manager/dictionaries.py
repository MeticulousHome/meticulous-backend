from enum import Enum

# Enumerations for the different types of controllers for easy access, validation and maintenance
class ReferenceType(Enum):
    TIME = "time"
    POSITION = "position"
    WEIGHT = "weight"

class Pressure_Algorithm_Type(Enum):
    PID_V1 = "pid v1"
    PID_V2 = "pid v2"
    
class Temperature_Algorithm_Type(Enum):
    WATER = "water"
    CYLINDER = "cylinder"
    TUBE = "tube"
    PLUNGER = "plunger"
    STABLE = "stable"
    
class Speed_Algorithm_Type(Enum):
    EASE_IN = "ease-in"
    FAST = "fast"

class CurveInterpolationType(Enum):
    LINEAR = "linear"
    CATMULL = "catmull"
    
class Message_Type(Enum):
    NO_WATER = "no water"
    REMOVE_CUP = "remove cup"
    PURGE = "purge"
    START_CLICK = "start click"
    PURGE_CLICK = "purge click"
    
class Direction_Type(Enum):
    FORWARD = "forward"
    BACKWARD = "backward"

controllers_type = {
    "power": "piston_power_controller",
    "flow" : "flow_controller",
    "pressure" : "pressure_controller",
    "weight" : "weight_controller",
    "speed" : "move_piston_controller",
    "temperature" : "temperature_controller",
    "tare" : "tare_controller",
    "message" : "log_controller",
    "end" : "end_profile",
    "weight" : "weight_controller",
    "end" : "end_profile"
}

algorithms_type = {
    "pressure" : {
        Pressure_Algorithm_Type.PID_V1 : "Pressure PID v1.0",
        Pressure_Algorithm_Type.PID_V2 : "Pressure PID v2.0",
    },
    "power" : "Spring v1.0",
    "temperature" : {
        Temperature_Algorithm_Type.WATER : "Water Temperature PID v1.0",
        Temperature_Algorithm_Type.CYLINDER : "Cylinder Temperature PID v1.0",
        Temperature_Algorithm_Type.TUBE: "Tube Temperature PID v1.0",
        Temperature_Algorithm_Type.PLUNGER : "Plunger Temperature PID v1.0",
        Temperature_Algorithm_Type.STABLE : "Stable Temperature"
    },
    "flow" : "Flow PID v1.0",
    "weight" : "Weight PID v1.0",
    "speed" : {
        Speed_Algorithm_Type.EASE_IN : "Piston Ease-In",
        Speed_Algorithm_Type.FAST : "Piston Fast",
    }
} 

reference_type = {
    "curve" : {
        ReferenceType.TIME : "time",
        ReferenceType.POSITION : "position",
        ReferenceType.WEIGHT : "weight"
    },
    "control" : {
        ReferenceType.TIME : "time_reference",
        ReferenceType.POSITION : "position_reference",
        ReferenceType.WEIGHT : "weight_reference"
    }
}

curve_interpolation = {
    CurveInterpolationType.LINEAR: "linear_interpolation",
    CurveInterpolationType.CATMULL : "catmull_interpolation"
}
        
messages = {
    Message_Type.NO_WATER : "No Water",
    Message_Type.REMOVE_CUP : "Remove Cup",
    Message_Type.PURGE : "Purge",
    Message_Type.START_CLICK : "Click to start",
    Message_Type.PURGE_CLICK : "Click to purge"
}

directions = {
    Direction_Type.FORWARD : "DOWN",
    Direction_Type.BACKWARD : "UP"
}
  
#! Triggers type dictionaries.

class TriggerType(Enum):
    FLOW = "flow"
    PRESSURE = "pressure"
    TEMPERATURE = "temperature"
    POWER = "power"
    
class SourceType(Enum):
    FLOW = "flow"
    PRESSURE = "pressure"
    WEIGHT = "weight"
    POWER = "power"
    RAW = "raw"
    AVERAGE = "average"
    PREDICTIVE = "predictive"
    TEMPERATURE = "temperature"
    BUTTON = "button"
    GESTURE = "button_gesture"
    
class ButtonSourceType(Enum):
    START = "start"
    TARE = "tare"
    ENCODER = "encoder"
    ENCODER_BUTTON = "encoder button"

class ButtonGestureSourceType(Enum):
    SINGLE = "single"
    DOUBLE = "double"
    RIGHT = "right"
    LEFT = "left"
    PRESSED = "pressed"
    RELEASED = "released"
    LONG = "long"
    
class TemperatureSourceType(Enum):
    TUBE = "tube"
    CYLINDER = "cylinder"
    PLUNGER = "plunger"
    WATER = "water"
    CYLINDER_AVERAGE = "cylinder average"
    
class TriggerOperatorType(Enum):
    GREATER_THAN = "greater_than"
    LESS_THAN = "less_than"
    EQUAL = "equal"
    GREATER_THAN_OR_EQUAL = "greater_than_or_equal"
    LESS_THAN_OR_EQUAL = "less_than_or_equal"

trigger_type = {
    "piston_position" : "piston_position_trigger",
    "speed" : "speed_trigger",
    "time" : "time_trigger",
    "weight" : "weight_trigger",
    "button" : "button_trigger",
    "water_detection" : "water_detection_trigger",
    "curve":{
        TriggerType.FLOW : "flow_curve_trigger",
        TriggerType.PRESSURE : "pressure_curve_trigger",
        TriggerType.TEMPERATURE : "temperature_curve_trigger",
        TriggerType.POWER : "power_curve_trigger",
    },
    "value" :{
        TriggerType.FLOW :"flow_value_trigger",
        TriggerType.PRESSURE : "pressure_value_trigger",
        TriggerType.TEMPERATURE: "temperature_value_trigger",
        TriggerType.POWER :  "power_value_trigger",
    },
    "exit" : "exit"
}

source_type = {
    SourceType.RAW : {
        SourceType.FLOW : "Flow Raw",
        SourceType.PRESSURE : "Pressure Raw",
        SourceType.WEIGHT : "Weight Raw",
        SourceType.POWER : "Piston Power Raw"
    },
    SourceType.AVERAGE : {
        SourceType.FLOW : "Flow Average",
        SourceType.PRESSURE : "Pressure Average",
        SourceType.WEIGHT : "Weight Average",
        SourceType.POWER : "Piston Power Average"
    },
    SourceType.PREDICTIVE : {
        SourceType.FLOW : "Flow Predictive",
        SourceType.PRESSURE : "Pressure Predictive",
        SourceType.WEIGHT  : "Weight Predictive",
        SourceType.POWER : "Piston Power Predictive"
    },
    SourceType.TEMPERATURE: {
        TemperatureSourceType.TUBE:"Tube Temperature", 
        TemperatureSourceType.CYLINDER:"Cylinder Temperature", 
        TemperatureSourceType.PLUNGER:"Plunger Temperature", 
        TemperatureSourceType.WATER:"Water Temperature", 
        TemperatureSourceType.CYLINDER_AVERAGE: "Cylinder Temperature Average"
    },
    SourceType.BUTTON: {
        ButtonSourceType.START : "Start Button",
        ButtonSourceType.TARE : "Tare Button",
        ButtonSourceType.ENCODER : "Encoder",
        ButtonSourceType.ENCODER_BUTTON : "Encoder Button"
        },
    
    SourceType.GESTURE:{ 
        ButtonGestureSourceType.SINGLE: "Single Tap",
        ButtonGestureSourceType.DOUBLE: "Double Tap",
        ButtonGestureSourceType.RIGHT: "Right",
        ButtonGestureSourceType.LEFT: "Left",
        ButtonGestureSourceType.PRESSED: "Pressed",
        ButtonGestureSourceType.RELEASED: "Released",
        ButtonGestureSourceType.LONG: "Long Press"  
    }
}

operator_type = {
    TriggerOperatorType.GREATER_THAN : ">",
    TriggerOperatorType.LESS_THAN: "<",
    TriggerOperatorType.EQUAL : "==",
    TriggerOperatorType.GREATER_THAN_OR_EQUAL: ">=",
    TriggerOperatorType.LESS_THAN_OR_EQUAL: "<="
}
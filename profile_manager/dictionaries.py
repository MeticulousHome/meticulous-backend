from enum import Enum

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
    "linear" : "linear_interpolation",
    "catmull" : "catmull_interpolation"
}
        
messages = {
    "no water" : "No Water",
    "remove cup" : "Remove Cup",
    "purge" : "Purge",
    "start click" : "Click to start",
    "purge click" : "Click to purge"
}

direction = {
    "forward" : "DOWN",
    "backward" : "UP"
}
  
from enum import Enum

# Enumerations for the different types of controllers for easy access, validation and maintenance
class ReferenceType(Enum):
    TIME = "time"
    POSITION = "position"
    WEIGHT = "weight"
    CURVE = "curve"
    CONTROL = "control"

class Pressure_Algorithm_Type(Enum):
    PID_V1 = "pid v1"
    PID_V2 = "pid v2"
    
class Temperature_Algorithm_Type(Enum):
    WATER = "water"
    CYLINDER = "cylinder"
    TUBE = "tube"
    PLUNGER = "plunger"
    STABLE = "stable"
    
class Power_Algorithm_Type(Enum):
    SPRING = "spring"
    
class Flow_Algorithm_Type(Enum):
    PID_V1 = "pid v1"
    
class Weight_Algorithm_Type(Enum):
    PID_V1 = "pid v1"    
    
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
    
class AlgorithmType(Enum):
    PRESSURE = "pressure"
    POWER = "power"
    TEMPERATURE = "temperature"
    FLOW = "flow"
    WEIGHT = "weight"
    SPEED = "speed"
    
class ControllerType(Enum):
    POWER = "power"
    FLOW = "flow"
    PRESSURE = "pressure"
    WEIGHT = "weight"
    SPEED = "speed"
    TEMPERATURE = "temperature"
    TARE = "tare"
    MESSAGE = "message"
    END = "end"
    
#! Triggers type dictionaries.

class TriggerType(Enum):
    PISTON_POSITION = "piston_position_trigger"
    SPEED = "speed_trigger"
    TIME = "time_trigger"
    WEIGHT = "weight_trigger"
    BUTTON = "button_trigger"
    WATER_DETECTION = "water_detection_trigger"
    EXIT = "exit"
    VALUE = "value"
    CURVE = "curve"
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
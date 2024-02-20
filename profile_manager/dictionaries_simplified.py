from dictionaries import *
from controllers import *

type_dict = {
    "power" : controllers_type[ControllerType.POWER],
    "flow" : controllers_type[ControllerType.FLOW],
    "pressure" : controllers_type[ControllerType.PRESSURE],
}

over_dict = {
    "time" : reference_type[ReferenceType.CURVE][ReferenceType.TIME],
    "weight" : reference_type[ReferenceType.CURVE][ReferenceType.WEIGHT],
    "piston_position" : reference_type[ReferenceType.CURVE][ReferenceType.POSITION],
}

interpolation_dict = {
    "linear" : curve_interpolation[CurveInterpolationType.LINEAR],
    "catmul" : curve_interpolation[CurveInterpolationType.CATMULL],
}

exit_trigger_dict = {
    "time" : trigger_type[TriggerType.TIME],
    "weight" : trigger_type[TriggerType.WEIGHT],
    "pressure" : trigger_type[TriggerType.VALUE][TriggerType.PRESSURE],
    "flow" : trigger_type[TriggerType.VALUE][TriggerType.FLOW],
    "piston_position" : trigger_type[TriggerType.PISTON_POSITION],
    "power" : trigger_type[TriggerType.VALUE][TriggerType.POWER],
    "temperature" : trigger_type[TriggerType.VALUE][TriggerType.TEMPERATURE],
}

limit_trigger_dict = {
    "pressure" : trigger_type[TriggerType.VALUE][TriggerType.PRESSURE],
    "flow" : trigger_type[TriggerType.VALUE][TriggerType.FLOW],
    "power" : trigger_type[TriggerType.VALUE][TriggerType.POWER],
    "temperature" : trigger_type[TriggerType.VALUE][TriggerType.TEMPERATURE],
}

if __name__ == '__main__':
    print(type_dict)
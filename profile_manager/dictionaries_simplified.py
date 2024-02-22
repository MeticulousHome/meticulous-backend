from dictionaries import *
from controllers import *
from triggers import *

type_dict = {
    "power" : PowerController,
    "flow" : FlowController,
    "pressure" : PressureController,
    "temperature" : TemperatureController,
}

over_dict = {
    "time" : ReferenceType.TIME,
    "weight" : ReferenceType.WEIGHT,
    "piston_position" : ReferenceType.POSITION,
}

interpolation_dict = {
    "linear" : CurveInterpolationType.LINEAR,
    "catmull" : CurveInterpolationType.CATMULL,
}

exit_trigger_dict = {
    "time" : TimerTrigger,
    "weight" : WeightTrigger,
    "pressure" : PressureValueTrigger,
    "flow" : FlowValueTrigger,
    "piston_position" : PistonPositionTrigger,
    "power" : PowerValueTrigger,
    "temperature" : TemperatureValueTrigger,
    "speed" : SpeedTrigger,
}

limit_trigger_dict = {
    "pressure" : trigger_type[TriggerType.VALUE][TriggerType.PRESSURE],
    "flow" : trigger_type[TriggerType.VALUE][TriggerType.FLOW],
    "power" : trigger_type[TriggerType.VALUE][TriggerType.POWER],
    "temperature" : trigger_type[TriggerType.VALUE][TriggerType.TEMPERATURE],
}

def create_controller(name, *args, **kwargs):
    if name in type_dict:
        controller_class = type_dict[name]
        return controller_class(*args, **kwargs)
    else:
        raise ValueError(f"No controller found with name: {name}")
    
def create_trigger(name, *args, **kwargs):
    if name in exit_trigger_dict:
        trigger_class = exit_trigger_dict[name]
        return trigger_class(*args, **kwargs)
    else:
        raise ValueError(f"No trigger found with name: {name}")

if __name__ == '__main__':
    # print(type_dict)
    flow_controller = create_controller("flow", algorithm=FlowAlgorithmType.PID_V1, curve_id=1, points=[0, 10])
    print(flow_controller.get_controller())
    
    weight_trigger = create_trigger("weight", source=SourceType.AVERAGE, operator=TriggerOperatorType.GREATER_THAN, value=10, weight_reference=1, next_node_id=2)
    print(weight_trigger.get_trigger())
    # print(flow_controller)

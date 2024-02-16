import json
from dictionaries import *
#This class is used to create the triggers for the complex JSON
class Triggers:
        
    def __init__(self):
        self.data = {}

    def set_next_node_id(self, node_id: int):
        self.data["next_node_id"] = node_id
        
    def get_trigger(self):
        return self.data    
               
class OperatorTriggers(Triggers):
    
    def __init__(self):
        self.data = {
            "kind": "",
            "operator": "",
            "value": 0,
        }            
    
    def set_kind(self, kind: TriggerType):
        if kind not in trigger_type:
            raise ValueError("Invalid trigger type")
        self.data["kind"] = trigger_type[kind]  
        
    def set_operator(self, operator: TriggerOperatorType):
        if operator not in operator_type:
            raise ValueError("Invalid trigger operator")
        self.data["operator"] = operator_type[operator]   
        
    def set_value(self, value: float):
        self.data["value"] = value   
        
class FlowValueTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.VALUE][TriggerType.FLOW]
        self.data["source"] = source_type[SourceType.RAW][SourceType.FLOW] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0
    
    def set_source(self, source: SourceType):
        if source not in source_type:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[source][SourceType.FLOW]

class PressureValueTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.VALUE][TriggerType.PRESSURE]
        self.data["source"] = source_type[SourceType.RAW][SourceType.PRESSURE] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0
        
    def set_source(self, source: SourceType):
        if source not in source_type:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[source][SourceType.PRESSURE]

class PowerValueTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.VALUE][TriggerType.POWER]
        self.data["source"] = source_type[SourceType.RAW][SourceType.POWER] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0
        
    def set_source(self, source: SourceType):
        if source not in source_type:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[source][SourceType.POWER]
            
class TemperatureValueTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.VALUE][TriggerType.TEMPERATURE]
        self.data["source"] = source_type[SourceType.TEMPERATURE][TemperatureSourceType.TUBE] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0
        
    def set_source(self, source: TemperatureSourceType):
        if source not in source_type[SourceType.TEMPERATURE]:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[SourceType.TEMPERATURE][source]
        
class PistonPositionTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.PISTON_POSITION]
        self.data["source"] = "Piston Position Raw"
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["position_reference_id"] = 0
        self.data["next_node_id"] = 0
        
    def set_position_reference_id(self, position_reference: int):
        self.data["position_reference_id"] = position_reference

class WeightTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.WEIGHT]
        self.data["source"] = source_type[SourceType.RAW][SourceType.WEIGHT] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["weight_reference_id"] = 0
        
    def set_weight_reference_id(self, weight_reference: int):
        self.data["weight_reference_id"] = weight_reference
        
    def set_source(self, source: SourceType):
        if source not in source_type:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[source][SourceType.WEIGHT]
class TimerTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.TIME]
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["timer_reference_id"] = 0
        
    def set_timer_reference_id(self, time_reference: int):
        self.data["timer_reference_id"] = time_reference
        
class CurveTriggers(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data = {
            "kind": "",
            "source": "",
            "operator": "",
            "curve_id": 0,
            "next_node_id": 0
        }
    
    def set_curve_id(self, curve_id: int):
        self.data["curve_id"] = curve_id

class FlowCurveTrigger(CurveTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.CURVE][TriggerType.FLOW]
        self.data["source"] = source_type[SourceType.RAW][SourceType.FLOW] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["curve_id"] = 0
        self.data["next_node_id"] = 0
        
    def set_source(self, source: SourceType):
        if source not in source_type:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[source][SourceType.FLOW]
        
class PressureCurveTrigger(CurveTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.CURVE][TriggerType.PRESSURE]
        self.data["source"] = source_type[SourceType.RAW][SourceType.PRESSURE] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["curve_id"] = 0
        self.data["next_node_id"] = 0
        
    def set_source(self, source: SourceType):
        if source not in source_type:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[source][SourceType.PRESSURE]

class PowerCurveTrigger(CurveTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.CURVE][TriggerType.POWER]
        self.data["source"] = source_type[SourceType.RAW][SourceType.POWER] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["curve_id"] = 0
        self.data["next_node_id"] = 0
        
    def set_source(self, source: SourceType):
        if source not in source_type:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[source][SourceType.POWER]
             
class TemperatureCurveTrigger(CurveTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.CURVE][TriggerType.TEMPERATURE]
        self.data["source"] = source_type[SourceType.TEMPERATURE][TemperatureSourceType.TUBE] 
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["curve_id"] = 0
        self.data["next_node_id"] = 0       
    
    def set_source(self, source: TemperatureSourceType):
        if source not in source_type[SourceType.TEMPERATURE]:
            raise ValueError("Invalid source kind")
        self.data["source"] = source_type[SourceType.TEMPERATURE][source]

class ButtonTrigger(Triggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.BUTTON]
        self.data["source"] = source_type[SourceType.BUTTON][ButtonSourceType.START]
        self.data["gesture"] = source_type[SourceType.GESTURE][ButtonGestureSourceType.SINGLE]
        self.data["next_node_id"] = 0
          
    def set_source(self, source: ButtonSourceType):
        if source not in source_type[SourceType.BUTTON]:
            raise ValueError("Invalid button source")
        self.data["source"] = source_type[SourceType.BUTTON][source]
    
    def set_gesture(self, gesture: ButtonGestureSourceType):
        if gesture not in source_type[SourceType.GESTURE]:
            raise ValueError("Invalid button gesture")
        self.data["gesture"] = source_type[SourceType.GESTURE][gesture]

class SpeedTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.SPEED]
        self.data["operator"] = operator_type[TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0
    
class ExitTrigger(Triggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.EXIT]
        self.data["next_node_id"] = 0
        
class WaterDetectionTrigger(Triggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[TriggerType.WATER_DETECTION]
        self.data["value"] = False
        self.data["next_node_id"] = 0
        
    def set_value(self, value: bool):
        self.data["value"] = value

            
if __name__ == "__main__":
    
    flow_value_trigger = FlowValueTrigger()
    flow_value_trigger.set_value(10)
    flow_value_trigger.set_next_node_id(1)
    flow_value_trigger.set_source(SourceType.RAW)
    flow_value_trigger.set_operator(TriggerOperatorType.GREATER_THAN_OR_EQUAL)
    print(json.dumps(flow_value_trigger.get_trigger(), indent=4))
    
    pressure_value_trigger = PressureValueTrigger()
    pressure_value_trigger.set_value(10)
    pressure_value_trigger.set_next_node_id(1)
    pressure_value_trigger.set_source(SourceType.AVERAGE)
    pressure_value_trigger.set_operator(TriggerOperatorType.LESS_THAN)
    print(json.dumps(pressure_value_trigger.get_trigger(), indent=4))
    
    power_value_trigger = PowerValueTrigger()
    power_value_trigger.set_value(10)
    power_value_trigger.set_next_node_id(1)
    power_value_trigger.set_source(SourceType.PREDICTIVE)
    power_value_trigger.set_operator(TriggerOperatorType.EQUAL)
    print(json.dumps(power_value_trigger.get_trigger(), indent=4))
    
    temperature_value_trigger = TemperatureValueTrigger()
    temperature_value_trigger.set_value(10)
    temperature_value_trigger.set_next_node_id(1)
    temperature_value_trigger.set_source(TemperatureSourceType.CYLINDER)
    temperature_value_trigger.set_operator(TriggerOperatorType.LESS_THAN_OR_EQUAL)
    print(json.dumps(temperature_value_trigger.get_trigger(), indent=4))
    
    piston_position_trigger = PistonPositionTrigger()
    piston_position_trigger.set_value(10)
    piston_position_trigger.set_position_reference_id(1)
    piston_position_trigger.set_next_node_id(1)
    print(json.dumps(piston_position_trigger.get_trigger(), indent=4))
    
    timer_trigger = TimerTrigger()
    timer_trigger.set_value(10)
    timer_trigger.set_timer_reference_id(1)
    timer_trigger.set_next_node_id(1)
    print(json.dumps(timer_trigger.get_trigger(), indent=4))
    
    weight_trigger = WeightTrigger()
    weight_trigger.set_value(10)
    weight_trigger.set_weight_reference_id(1)
    weight_trigger.set_next_node_id(1)
    weight_trigger.set_source(SourceType.PREDICTIVE)
    print(json.dumps(weight_trigger.get_trigger(), indent=4))
    
    flow_curve_trigger = FlowCurveTrigger()
    flow_curve_trigger.set_curve_id(1)
    flow_curve_trigger.set_next_node_id(1)
    flow_curve_trigger.set_source(SourceType.AVERAGE)
    print(json.dumps(flow_curve_trigger.get_trigger(), indent=4))
    
    pressure_curve_trigger = PressureCurveTrigger()
    pressure_curve_trigger.set_curve_id(1)
    pressure_curve_trigger.set_next_node_id(1)
    pressure_curve_trigger.set_source(SourceType.RAW)
    print(json.dumps(pressure_curve_trigger.get_trigger(), indent=4))
    
    power_curve_trigger = PowerCurveTrigger()
    power_curve_trigger.set_curve_id(1)
    power_curve_trigger.set_next_node_id(1)
    power_curve_trigger.set_source(SourceType.PREDICTIVE)
    print(json.dumps(power_curve_trigger.get_trigger(), indent=4))
    
    temperature_curve_trigger = TemperatureCurveTrigger()
    temperature_curve_trigger.set_curve_id(1)
    temperature_curve_trigger.set_next_node_id(1)
    temperature_curve_trigger.set_source(TemperatureSourceType.TUBE)
    print(json.dumps(temperature_curve_trigger.get_trigger(), indent=4))
    
    button_trigger = ButtonTrigger()
    button_trigger.set_source(ButtonSourceType.START)
    button_trigger.set_gesture(ButtonGestureSourceType.SINGLE)
    button_trigger.set_next_node_id(1)
    print(json.dumps(button_trigger.get_trigger(), indent=4))
    
    exit_trigger = ExitTrigger()
    exit_trigger.set_next_node_id(1)
    print(json.dumps(exit_trigger.get_trigger(), indent=4))
    
    water_detection_trigger = WaterDetectionTrigger()
    water_detection_trigger.set_value(True)
    water_detection_trigger.set_next_node_id(1)
    print(json.dumps(water_detection_trigger.get_trigger(), indent=4))
    
    speed_trigger = SpeedTrigger()
    speed_trigger.set_value(10)
    speed_trigger.set_next_node_id(1)
    print(json.dumps(speed_trigger.get_trigger(), indent=4))
    
    
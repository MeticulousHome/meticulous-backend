import json
from dictionaries import trigger_type, source_type , operator_type
import dictionaries as dic
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
    
    def set_kind(self, kind: dic.enums.TriggerType):
        if kind not in trigger_type:
            raise ValueError("Invalid trigger type")
        self.data["kind"] = trigger_type[kind]  
        
    def set_operator(self, operator: dic.enums.TriggerOperatorType):
        if operator not in operator_type:
            raise ValueError("Invalid trigger operator")
        self.data["operator"] = operator_type[operator]   
        
    def set_value(self, value: float):
        self.data["value"] = value   
 
class ValueTriggers(OperatorTriggers):
    def __init__(self):
        self.data = {
            "kind": "",
            "source": "",
            "operator": "",
            "value": 0,
        } 
    
    def set_source(self, source_kind: dic.enums.SourceType, source_type: dic.enums.SourceType):
        if source_kind not in source_type:
            raise ValueError("Invalid source kind")
        if source_type not in source_type[source_kind]:
            raise ValueError("Invalid source type")
        self.data["source"] = source_type[source_kind][source_type] 
        
class FlowValueTrigger(ValueTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.VALUE][dic.enums.TriggerType.FLOW]
        self.data["source"] = source_type[dic.enums.SourceType.RAW][dic.enums.SourceType.FLOW] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0

class PressureValueTrigger(ValueTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.VALUE][dic.enums.TriggerType.PRESSURE]
        self.data["source"] = source_type[dic.enums.SourceType.RAW][dic.enums.SourceType.PRESSURE] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0

class PowerValueTrigger(ValueTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.VALUE][dic.enums.TriggerType.POWER]
        self.data["source"] = source_type[dic.enums.SourceType.RAW][dic.enums.SourceType.POWER] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0
            
class TemperatureValueTrigger(ValueTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.VALUE][dic.enums.TriggerType.TEMPERATURE]
        self.data["source"] = source_type[dic.enums.SourceType.TEMPERATURE][dic.enums.TemperatureSourceType.TUBE] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0
        
class PistonPositionTrigger(ValueTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.PISTON_POSITION]
        self.data["source"] = "Piston Position Raw"
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["position_reference_id"] = 0
        self.data["next_node_id"] = 0
        
    def set_position_reference_id(self, position_reference: int):
        self.data["position_reference_id"] = position_reference

class WeightTrigger(ValueTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.WEIGHT]
        self.data["source"] = source_type[dic.enums.SourceType.RAW][dic.enums.SourceType.WEIGHT] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["weight_reference_id"] = 0
        
    def set_weight_reference_id(self, weight_reference: int):
        self.data["weight_reference_id"] = weight_reference
class TimerTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.TIME]
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
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
        
    def set_source(self, source_kind: dic.enums.SourceType, source_type: dic.enums.SourceType):
        if source_kind not in source_type:
            raise ValueError("Invalid source kind")
        if source_type not in source_type[source_kind]:
            raise ValueError("Invalid source type")
        self.data["source"] = source_type[source_kind][source_type]
    
    def set_curve_id(self, curve_id: int):
        self.data["curve_id"] = curve_id

class FlowCurveTrigger(CurveTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.CURVE][dic.enums.TriggerType.FLOW]
        self.data["source"] = source_type[dic.enums.SourceType.RAW][dic.enums.SourceType.FLOW] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["curve_id"] = 0
        self.data["next_node_id"] = 0
        
class PressureCurveTrigger(CurveTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.CURVE][dic.enums.TriggerType.PRESSURE]
        self.data["source"] = source_type[dic.enums.SourceType.RAW][dic.enums.SourceType.PRESSURE] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["curve_id"] = 0
        self.data["next_node_id"] = 0

class PowerCurveTrigger(CurveTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.CURVE][dic.enums.TriggerType.POWER]
        self.data["source"] = source_type[dic.enums.SourceType.RAW][dic.enums.SourceType.POWER] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["curve_id"] = 0
        self.data["next_node_id"] = 0
            
class TemperatureCurveTrigger(CurveTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.CURVE][dic.enums.TriggerType.TEMPERATURE]
        self.data["source"] = source_type[dic.enums.SourceType.TEMPERATURE][dic.enums.TemperatureSourceType.TUBE] 
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["curve_id"] = 0
        self.data["next_node_id"] = 0       
    
    

class ButtonTrigger(Triggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.BUTTON]
        self.data["source"] = source_type[dic.enums.SourceType.BUTTON][dic.enums.ButtonSourceType.START]
        self.data["gesture"] = source_type[dic.enums.SourceType.GESTURE][dic.enums.ButtonGestureSourceType.SINGLE]
        self.data["next_node_id"] = 0
          
    def set_source(self, source: dic.enums.ButtonSourceType):
        if source not in source_type[dic.enums.SourceType.BUTTON]:
            raise ValueError("Invalid button source")
        self.data["source"] = source_type[dic.enums.SourceType.BUTTON][source]
    
    def set_gesture(self, gesture: dic.enums.ButtonGestureSourceType):
        if gesture not in source_type[dic.enums.SourceType.GESTURE]:
            raise ValueError("Invalid button gesture")
        self.data["gesture"] = source_type[dic.enums.SourceType.GESTURE][gesture]

class SpeedTrigger(OperatorTriggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.SPEED]
        self.data["operator"] = operator_type[dic.enums.TriggerOperatorType.GREATER_THAN]
        self.data["value"] = 0
        self.data["next_node_id"] = 0
    
class ExitTrigger(Triggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.EXIT]
        self.data["next_node_id"] = 0
        
class WaterDetectionTrigger(Triggers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = trigger_type[dic.enums.TriggerType.WATER_DETECTION]
        self.data["value"] = False
        self.data["next_node_id"] = 0
        
    def set_value(self, value: bool):
        self.data["value"] = value

            
if __name__ == "__main__":
    
    flow_value_trigger = FlowValueTrigger()
    flow_value_trigger.set_value(10)
    flow_value_trigger.set_next_node_id(1)
    print(json.dumps(flow_value_trigger.get_trigger(), indent=4))
    
    pressure_value_trigger = PressureValueTrigger()
    pressure_value_trigger.set_value(10)
    pressure_value_trigger.set_next_node_id(1)
    print(json.dumps(pressure_value_trigger.get_trigger(), indent=4))
    
    power_value_trigger = PowerValueTrigger()
    power_value_trigger.set_value(10)
    power_value_trigger.set_next_node_id(1)
    print(json.dumps(power_value_trigger.get_trigger(), indent=4))
    
    temperature_value_trigger = TemperatureValueTrigger()
    temperature_value_trigger.set_value(10)
    temperature_value_trigger.set_next_node_id(1)
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
    print(json.dumps(weight_trigger.get_trigger(), indent=4))
    
    flow_curve_trigger = FlowCurveTrigger()
    flow_curve_trigger.set_curve_id(1)
    flow_curve_trigger.set_next_node_id(1)
    print(json.dumps(flow_curve_trigger.get_trigger(), indent=4))
    
    pressure_curve_trigger = PressureCurveTrigger()
    pressure_curve_trigger.set_curve_id(1)
    pressure_curve_trigger.set_next_node_id(1)
    print(json.dumps(pressure_curve_trigger.get_trigger(), indent=4))
    
    power_curve_trigger = PowerCurveTrigger()
    power_curve_trigger.set_curve_id(1)
    power_curve_trigger.set_next_node_id(1)
    print(json.dumps(power_curve_trigger.get_trigger(), indent=4))
    
    temperature_curve_trigger = TemperatureCurveTrigger()
    temperature_curve_trigger.set_curve_id(1)
    temperature_curve_trigger.set_next_node_id(1)
    print(json.dumps(temperature_curve_trigger.get_trigger(), indent=4))
    
    button_trigger = ButtonTrigger()
    button_trigger.set_source(dic.enums.ButtonSourceType.START)
    button_trigger.set_gesture(dic.enums.ButtonGestureSourceType.SINGLE)
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
    
    
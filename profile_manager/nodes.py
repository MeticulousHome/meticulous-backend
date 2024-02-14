import controllers
import dictionaries as dic
import triggers
import json

class Nodes:
    def __init__(self):
        self.data = {}
        self.data["id"] = 0
        self.data["controllers"] = []
        self.data["triggers"] = []
    
    def set_id(self, id: int):
        self.data["id"] = id
        
    def add_controller(self, controller: controllers.Controllers):
        self.data["controllers"].append(controller.get_controller())
    
    def add_trigger(self, trigger: triggers.Triggers):
        self.data["triggers"].append(trigger.get_trigger()) 

    def get_node(self):
        return self.data    
    

if __name__ == "__main__":
    node = Nodes()
    node.set_id(1)
    points = [[0, 6],[10,8]]
     
    controller = controllers.FlowController()
    controller.set_curve_id(1)
    controller.set_interpolation_kind(dic.enums.CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(dic.enums.ReferenceType.TIME)
    controller.set_reference_id(1)
    node.add_controller(controller)
    
    controller = controllers.TimeReferenceController()
    controller.set_reference_id(2)
    node.add_controller(controller)
    
    trigger = triggers.SpeedTrigger()
    trigger.set_value(1)
    trigger.set_next_node_id(1)
    node.add_trigger(trigger)
    
    node_2 = Nodes()
    
    node_2.set_id(2)
    points = [[10, 15],[20,18]]
    
    controller = controllers.PressureController()
    controller.set_algorithm(dic.enums.Pressure_Algorithm_Type.PID_V1)
    controller.set_curve_id(2)
    controller.set_interpolation_kind(dic.enums.CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(dic.enums.ReferenceType.POSITION)
    controller.set_reference_id(2)
    node_2.add_controller(controller)
    
    controller = controllers.WeightController()
    controller.set_curve_id(3)
    controller.set_interpolation_kind(dic.enums.CurveInterpolationType.CATMULL)
    controller.set_points(points)
    controller.set_reference_type(dic.enums.ReferenceType.WEIGHT)
    controller.set_reference_id(3)
    node_2.add_controller(controller)
    
    controller = controllers.TemperatureController()
    controller.set_algorithm(dic.enums.Temperature_Algorithm_Type.WATER)
    controller.set_curve_id(4)
    controller.set_interpolation_kind(dic.enums.CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(dic.enums.ReferenceType.TIME)
    controller.set_reference_id(4)
    node_2.add_controller(controller)
    
    trigger = triggers.WeightTrigger()
    trigger.set_value(2)
    trigger.set_weight_reference_id(200)
    trigger.set_next_node_id(2)
    node_2.add_trigger(trigger)
    
    trigger = triggers.TimerTrigger()
    trigger.set_value(3)
    trigger.set_timer_reference_id(300)
    trigger.set_next_node_id(3)
    node_2.add_trigger(trigger)
    
    trigger = triggers.PressureCurveTrigger()
    trigger.set_curve_id(5)
    trigger.set_next_node_id(4)
    node_2.add_trigger(trigger)
    
    trigger = triggers.TemperatureValueTrigger()
    trigger.set_value(5)
    trigger.set_next_node_id(5)
    node_2.add_trigger(trigger)
    
    trigger = triggers.ButtonTrigger()
    trigger.set_source(dic.enums.ButtonSourceType.ENCODER_BUTTON)
    trigger.set_gesture(dic.enums.ButtonGestureSourceType.SINGLE)
    trigger.set_next_node_id(6)
    node_2.add_trigger(trigger)
    
    nodes = [node.get_node(), node_2.get_node()]
    print(json.dumps(nodes, indent=4))
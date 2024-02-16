from nodes import *
import json

class Stages:
    def __init__(self, name: str = ""):
        self.data = {}
        self.data["name"] = name
        self.data["nodes"] = []
        
    def set_name(self, name: str):
        self.data["name"] = name
    
    def add_node(self, node: Nodes):
        self.data["nodes"].append(node.get_node())
        
    def get_stage(self):
        return self.data
    
if __name__ == "__main__":
    stage_1 = Stages("Stage 1")
    
    node = Nodes()
    node.set_id(1)
    points = [[0, 6],[10,8]]
    
    controller = FlowController()
    controller.set_curve_id(1)
    controller.set_interpolation_kind(CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(ReferenceType.TIME)
    controller.set_reference_id(1)
    node.add_controller(controller)
    
    controller = TimeReferenceController()
    controller.set_reference_id(2)
    node.add_controller(controller)

    trigger = SpeedTrigger()
    trigger.set_value(1)
    trigger.set_operator(TriggerOperatorType.GREATER_THAN_OR_EQUAL)
    trigger.set_next_node_id(1)
    node.add_trigger(trigger)
    
    node_1 = Nodes()
    node_1.set_id(2)
    points = [[10, 15],[20,18]]
    
    controller = PressureController()
    controller.set_algorithm(Pressure_Algorithm_Type.PID_V1)
    controller.set_curve_id(2)
    controller.set_interpolation_kind(CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(ReferenceType.POSITION)
    controller.set_reference_id(2)
    node_1.add_controller(controller)
    
    stage_1.add_node(node)
    stage_1.add_node(node_1)
    
    stage_2 = Stages()
    stage_2.set_name("Stage 2")
    
    node_2 = Nodes()
    node_2.set_id(3)
    points = [[20, 25],[30,28]]
    
    controller = PressureController()
    controller.set_algorithm(Pressure_Algorithm_Type.PID_V1)
    controller.set_curve_id(3)
    controller.set_interpolation_kind(CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(ReferenceType.POSITION)
    controller.set_reference_id(3)
    node_2.add_controller(controller)
    
    controller = TareController()
    node_2.add_controller(controller)
    
    trigger = SpeedTrigger()
    trigger.set_value(2)
    trigger.set_next_node_id(3)
    node_2.add_trigger(trigger)
    
    node_3 = Nodes()
    node_3.set_id(4)
    points = [[30, 35],[40,38]]
    
    controller = TimeReferenceController()
    controller.set_reference_id(4)
    node_3.add_controller(controller)
    
    controller = WeightController()
    controller.set_curve_id(4)
    controller.set_interpolation_kind(CurveInterpolationType.CATMULL)
    controller.set_points(points)
    controller.set_reference_type(ReferenceType.WEIGHT)
    controller.set_reference_id(4)
    node_3.add_controller(controller)
    
    trigger = WeightTrigger()
    trigger.set_value(3)
    trigger.set_weight_reference_id(200)
    trigger.set_next_node_id(4)
    node_3.add_trigger(trigger)
    
    trigger = ExitTrigger()
    # trigger.set_value(4)
    trigger.set_next_node_id(1)
    node_3.add_trigger(trigger)
    
    stage_2.add_node(node_2)
    stage_2.add_node(node_3)
    
    stages = [stage_1.get_stage(), stage_2.get_stage()]
    
    print(json.dumps(stages, indent=4))
    
    
    # print(json.dumps(stage_1.get_stage(), indent=4))
        
    
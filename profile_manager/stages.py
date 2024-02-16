import nodes 
import json

class Stages:
    def __init__(self):
        self.data = {}
        self.data["name"] = ""
        self.data["nodes"] = []
        
    def set_name(self, name: str):
        self.data["name"] = name
    
    def add_node(self, node: nodes.Nodes):
        self.data["nodes"].append(node.get_node())
        
    def get_stage(self):
        return self.data
    
if __name__ == "__main__":
    stage_1 = Stages()
    stage_1.set_name("Stage 1")
    
    node = nodes.Nodes()
    node.set_id(1)
    points = [[0, 6],[10,8]]
    
    controller = nodes.controllers.FlowController()
    controller.set_curve_id(1)
    controller.set_interpolation_kind(nodes.dic.enums.CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(nodes.dic.enums.ReferenceType.TIME)
    controller.set_reference_id(1)
    node.add_controller(controller)
    
    controller = nodes.controllers.TimeReferenceController()
    controller.set_reference_id(2)
    node.add_controller(controller)

    trigger = nodes.triggers.SpeedTrigger()
    trigger.set_value(1)
    trigger.set_operator(nodes.dic.enums.TriggerOperatorType.GREATER_THAN_OR_EQUAL)
    trigger.set_next_node_id(1)
    node.add_trigger(trigger)
    
    node_1 = nodes.Nodes()
    node_1.set_id(2)
    points = [[10, 15],[20,18]]
    
    controller = nodes.controllers.PressureController()
    controller.set_algorithm(nodes.dic.enums.Pressure_Algorithm_Type.PID_V1)
    controller.set_curve_id(2)
    controller.set_interpolation_kind(nodes.dic.enums.CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(nodes.dic.enums.ReferenceType.POSITION)
    controller.set_reference_id(2)
    node_1.add_controller(controller)
    
    stage_1.add_node(node)
    stage_1.add_node(node_1)
    
    stage_2 = Stages()
    stage_2.set_name("Stage 2")
    
    node_2 = nodes.Nodes()
    node_2.set_id(3)
    points = [[20, 25],[30,28]]
    
    controller = nodes.controllers.PressureController()
    controller.set_algorithm(nodes.dic.enums.Pressure_Algorithm_Type.PID_V1)
    controller.set_curve_id(3)
    controller.set_interpolation_kind(nodes.dic.enums.CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(nodes.dic.enums.ReferenceType.POSITION)
    controller.set_reference_id(3)
    node_2.add_controller(controller)
    
    controller = nodes.controllers.TareController()
    node_2.add_controller(controller)
    
    trigger = nodes.triggers.SpeedTrigger()
    trigger.set_value(2)
    trigger.set_next_node_id(3)
    node_2.add_trigger(trigger)
    
    node_3 = nodes.Nodes()
    node_3.set_id(4)
    points = [[30, 35],[40,38]]
    
    controller = nodes.controllers.TimeReferenceController()
    controller.set_reference_id(4)
    node_3.add_controller(controller)
    
    controller = nodes.controllers.WeightController()
    controller.set_curve_id(4)
    controller.set_interpolation_kind(nodes.dic.enums.CurveInterpolationType.CATMULL)
    controller.set_points(points)
    controller.set_reference_type(nodes.dic.enums.ReferenceType.WEIGHT)
    controller.set_reference_id(4)
    node_3.add_controller(controller)
    
    trigger = nodes.triggers.WeightTrigger()
    trigger.set_value(3)
    trigger.set_weight_reference_id(200)
    trigger.set_next_node_id(4)
    node_3.add_trigger(trigger)
    
    trigger = nodes.triggers.ExitTrigger()
    # trigger.set_value(4)
    trigger.set_next_node_id(1)
    node_3.add_trigger(trigger)
    
    stage_2.add_node(node_2)
    stage_2.add_node(node_3)
    
    stages = [stage_1.get_stage(), stage_2.get_stage()]
    
    print(json.dumps(stages, indent=4))
    
    
    # print(json.dumps(stage_1.get_stage(), indent=4))
        
    
from .nodes import *
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
    
    node = Nodes(1)
    points = [[0, 6],[10,8]]
    
    controller = FlowController(FlowAlgorithmType.PID_V1, 1, CurveInterpolationType.LINEAR, points, ReferenceType.TIME, 1)
    node.add_controller(controller)
    
    controller = TimeReferenceController(2)
    node.add_controller(controller)

    trigger = SpeedTrigger(TriggerOperatorType.GREATER_THAN_OR_EQUAL, 10, 2)
    node.add_trigger(trigger)
    
    node_1 = Nodes(2)
    points = [[10, 15],[20,18]]
    
    controller = PressureController(PressureAlgorithmType.PID_V1, 4, CurveInterpolationType.CATMULL, points, ReferenceType.POSITION, 2)
    node_1.add_controller(controller)
    
    stage_1.add_node(node)
    stage_1.add_node(node_1)
    
    stage_2 = Stages()
    stage_2.set_name("Stage 2")
    
    node_2 = Nodes(3)
    points = [[20, 25],[30,28]]
    
    controller = PressureController(PressureAlgorithmType.PID_V1, 5, CurveInterpolationType.CATMULL, points, ReferenceType.POSITION, 3)
    node_2.add_controller(controller)
    
    controller = TareController()
    node_2.add_controller(controller)
    
    trigger = SpeedTrigger(TriggerOperatorType.GREATER_THAN_OR_EQUAL, 20, 3)
    node_2.add_trigger(trigger)
    
    node_3 = Nodes()
    node_3.set_id(4)
    points = [[30, 35],[40,38]]
    
    controller = TimeReferenceController()
    controller.set_reference_id(4)
    node_3.add_controller(controller)
    
    controller = WeightController(WeightAlgorithmType.PID_V1, 200, CurveInterpolationType.LINEAR, points, ReferenceType.WEIGHT, 4)
    node_3.add_controller(controller)
    
    trigger = WeightTrigger(SourceType.AVERAGE, TriggerOperatorType.GREATER_THAN_OR_EQUAL,3, 200, 4)
    node_3.add_trigger(trigger)
    
    trigger = ExitTrigger(1)
    node_3.add_trigger(trigger)
    
    stage_2.add_node(node_2)
    stage_2.add_node(node_3)
    
    stages = [stage_1.get_stage(), stage_2.get_stage()]
    
    print(json.dumps(stages, indent=4))
    
    
    # print(json.dumps(stage_1.get_stage(), indent=4))
        
    
import json 
from stages import *

class Profile:
    def __init__(self, name: str = "", complex_stages: list = None):
        
        if complex_stages is None:
            complex_stages = []
        self.data = {
            "name": name,
            "stages": complex_stages
        }
        
    def set_name(self, name: str):
        self.data["name"] = name
    
    def add_stages(self, stages: list):
        for stage in stages:
            self.data["stages"].append(stage.get_stage())

    def get_data(self):
        return self.data
    
if __name__ == "__main__":
    profile = Profile("Profile 1")
    
    
    stages_1 = Stages()
    stages_1.set_name("Stage 1")
    
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
    trigger.set_next_node_id(1)
    node.add_trigger(trigger)
    
    stages_1.add_node(node)
    
    node_1 = Nodes()
    node_1.set_id(2)
    points = [[10, 15],[20,18]]
    
    controller = PressureController()
    controller.set_algorithm(PressureAlgorithmType.PID_V1)
    controller.set_curve_id(2)
    controller.set_interpolation_kind(CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(ReferenceType.POSITION)
    controller.set_reference_id(2)
    node_1.add_controller(controller)
    
    stages_1.add_node(node_1)
    
    stages_2 = Stages()
    stages_2.set_name("Stage 2")
    
    node_2 = Nodes()
    node_2.set_id(3)
    points = [[20, 25],[30,28]]
    
    controller = PressureController()
    controller.set_algorithm(PressureAlgorithmType.PID_V1)
    controller.set_curve_id(3)
    controller.set_interpolation_kind(CurveInterpolationType.LINEAR)
    controller.set_points(points)
    controller.set_reference_type(ReferenceType.POSITION)
    controller.set_reference_id(3)
    node_2.add_controller(controller)
    
    trigger = SpeedTrigger()
    trigger.set_value(2)
    trigger.set_next_node_id(3)
    node_2.add_trigger(trigger)
    
    stages_2.add_node(node_2)
    
    profile.add_stages([stages_1,stages_2])
    
    print(json.dumps(profile.get_data(), indent=2))
    
    
    
        
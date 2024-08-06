import json
from .controllers import *
from .triggers import *
from .dictionaries import *


class Nodes:
    def __init__(self, id: int = 0):
        self.data = {}
        self.data["id"] = id
        self.data["controllers"] = []
        self.data["triggers"] = []

    def set_id(self, id: int):
        self.data["id"] = id

    def add_controller(self, controller: Controllers):
        self.data["controllers"].append(controller.get_controller())

    def add_trigger(self, trigger: Triggers):
        self.data["triggers"].append(trigger.get_trigger())

    def get_node_id(self):
        return self.data["id"]

    def get_node(self):
        return self.data


if __name__ == "__main__":
    node = Nodes(1)
    # node.set_id(1)
    points = [[0, 6], [10, 8]]

    controller = FlowController(
        FlowAlgorithmType.PID_V1,
        1,
        CurveInterpolationType.LINEAR,
        points,
        ReferenceType.TIME,
        1,
    )
    node.add_controller(controller)

    controller = TimeReferenceController(100)
    node.add_controller(controller)

    trigger = SpeedTrigger(TriggerOperatorType.GREATER_THAN_OR_EQUAL, 10, 12)
    node.add_trigger(trigger)

    node_2 = Nodes(2)
    points = [[10, 15], [20, 18]]

    controller = PressureController(
        PressureAlgorithmType.PID_V1,
        7,
        CurveInterpolationType.CATMULL,
        points,
        ReferenceType.POSITION,
        2,
    )
    node_2.add_controller(controller)

    controller = WeightController(
        WeightAlgorithmType.PID_V1,
        2,
        CurveInterpolationType.LINEAR,
        points,
        ReferenceType.WEIGHT,
        3,
    )
    node_2.add_controller(controller)

    controller = TemperatureController(
        TemperatureAlgorithmType.WATER,
        4,
        CurveInterpolationType.CATMULL,
        points,
        ReferenceType.TIME,
        4,
    )
    node_2.add_controller(controller)

    trigger = WeightTrigger(SourceType.AVERAGE, TriggerOperatorType.GREATER_THAN, 10, 1)
    node_2.add_trigger(trigger)

    trigger = TimerTrigger(TriggerOperatorType.GREATER_THAN, 10, 2)
    node_2.add_trigger(trigger)

    trigger = PressureCurveTrigger(
        SourceType.AVERAGE, TriggerOperatorType.GREATER_THAN_OR_EQUAL, 1, 4
    )
    node_2.add_trigger(trigger)

    trigger = TemperatureValueTrigger(
        TemperatureSourceType.WATER, TriggerOperatorType.GREATER_THAN, 10, 3
    )
    node_2.add_trigger(trigger)

    trigger = ButtonTrigger(
        ButtonSourceType.ENCODER_BUTTON, ButtonGestureSourceType.SINGLE, 6
    )
    node_2.add_trigger(trigger)

    nodes = [node.get_node(), node_2.get_node()]
    print(json.dumps(nodes, indent=4))

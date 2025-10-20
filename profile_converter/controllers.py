import json

from profile_converter.dictionaries import (
    algorithms_type,
    controllers_type,
    curve_interpolation,
    directions,
    messages,
    reference_type,
)
from profile_converter.enums import (
    AlgorithmType,
    ControllerType,
    CurveInterpolationType,
    DirectionType,
    FlowAlgorithmType,
    MessageType,
    PowerAlgorithmType,
    PressureAlgorithmType,
    ReferenceType,
    SpeedAlgorithmType,
    TemperatureAlgorithmType,
    WeightAlgorithmType,
)


class Controllers:
    # This parent class has the get_controller method that returns a dictionary with the information of the controllers

    def __init__(self):
        self.data = {}

    def get_controller(self):
        return self.data


class CurveControllers(Controllers):
    """
    This child class of Controllers is a special class made for controllers that have an associated curve.

    Attributes:

    set_curve_id: int -> ID of the curve associated with the controller
    set_interpolation_kind: str -> type of curve interpolation
    set_points: list -> list of points of the curve
    set_reference_type: str -> type of curve reference
    set_reference_id: int -> ID of the curve reference
    self.data: dict -> dictionary with the information of the controller and its associated curve
    """

    def __init__(self):
        self.data = {
            "kind": "",
            "algorithm": "",
            "curve": {
                "id": 0,
                "interpolation_kind": "",
                "points": [],
                "reference": {"kind": "", "id": 0},
            },
        }

    def set_curve_id(self, id: int):
        self.data["curve"]["id"] = id

    def set_interpolation_kind(self, interpolation_kind: CurveInterpolationType):
        if interpolation_kind not in curve_interpolation:
            raise ValueError("Invalid interpolation kind")

        self.data["curve"]["interpolation_kind"] = curve_interpolation[interpolation_kind]

    def set_points(self, points: list):
        self.data["curve"]["points"].extend(points)

    def set_reference_type(self, reference_kind: ReferenceType):
        if reference_kind not in reference_type[ReferenceType.CURVE]:
            raise ValueError("Invalid reference kind")
        self.data["curve"]["reference"]["kind"] = reference_type[ReferenceType.CURVE][
            reference_kind
        ]

    def set_reference_id(self, reference_id: int):
        self.data["curve"]["reference"]["id"] = reference_id

    def get_curve_id(self):
        return self.data["curve"]["id"]


"""
Child classes of CurveControllers that represent pressure, flow, temperature, power, and weight controllers
These classes have methods to change the controller's algorithm and to change the curve associated with the controller
"""


class PressureController(CurveControllers):
    def __init__(
        self,
        algorithm: PressureAlgorithmType = PressureAlgorithmType.PID_V1,
        curve_id: int = 0,
        interpolation_kind: CurveInterpolationType = CurveInterpolationType.LINEAR,
        points: list = [0, 6],
        reference_kind: ReferenceType = ReferenceType.TIME,
        reference_id: int = 0,
    ):
        super().__init__()
        self.data["kind"] = controllers_type[ControllerType.PRESSURE]
        self.data["algorithm"] = algorithms_type[AlgorithmType.PRESSURE][algorithm]
        self.data["curve"]["id"] = curve_id
        self.data["curve"]["interpolation_kind"] = curve_interpolation[interpolation_kind]
        self.data["curve"]["points"] = points
        self.data["curve"]["reference"]["kind"] = reference_type[ReferenceType.CURVE][
            reference_kind
        ]
        self.data["curve"]["reference"]["id"] = reference_id

    def set_algorithm(self, algorithm: PressureAlgorithmType):
        # only accept valid algorithms
        if algorithm not in algorithms_type[AlgorithmType.PRESSURE]:
            raise ValueError("Invalid algorithm")

        self.data["algorithm"] = algorithms_type[AlgorithmType.PRESSURE][algorithm]


class FlowController(CurveControllers):
    def __init__(
        self,
        algorithm: FlowAlgorithmType = FlowAlgorithmType.PID_V1,
        curve_id: int = 0,
        interpolation_kind: CurveInterpolationType = CurveInterpolationType.LINEAR,
        points: list = [0, 8],
        reference_kind: ReferenceType = ReferenceType.TIME,
        reference_id: int = 0,
    ):
        super().__init__()
        self.data["kind"] = controllers_type[ControllerType.FLOW]
        self.data["algorithm"] = algorithms_type[AlgorithmType.FLOW][algorithm]
        self.data["curve"]["id"] = curve_id
        self.data["curve"]["interpolation_kind"] = curve_interpolation[interpolation_kind]
        self.data["curve"]["points"] = points
        self.data["curve"]["reference"]["kind"] = reference_type[ReferenceType.CURVE][
            reference_kind
        ]
        self.data["curve"]["reference"]["id"] = reference_id

    def set_algorithm(self, algorithm: FlowAlgorithmType):
        if algorithm not in algorithms_type[AlgorithmType.FLOW]:
            raise ValueError("Invalid algorithm")

        self.data["algorithm"] = algorithms_type[AlgorithmType.FLOW][algorithm]


class TemperatureController(CurveControllers):
    def __init__(
        self,
        algorithm: TemperatureAlgorithmType = TemperatureAlgorithmType.WATER,
        curve_id: int = 0,
        interpolation_kind: CurveInterpolationType = CurveInterpolationType.LINEAR,
        points: list = [0, 8],
        reference_kind: ReferenceType = ReferenceType.TIME,
        reference_id: int = 0,
    ):
        super().__init__()
        self.data["kind"] = controllers_type[ControllerType.TEMPERATURE]
        self.data["algorithm"] = algorithms_type[AlgorithmType.TEMPERATURE][algorithm]
        self.data["curve"]["id"] = curve_id
        self.data["curve"]["interpolation_kind"] = curve_interpolation[interpolation_kind]
        self.data["curve"]["points"] = points
        self.data["curve"]["reference"]["kind"] = reference_type[ReferenceType.CURVE][
            reference_kind
        ]
        self.data["curve"]["reference"]["id"] = reference_id

    def set_algorithm(self, algorithm: TemperatureAlgorithmType):
        if algorithm not in algorithms_type[AlgorithmType.TEMPERATURE]:
            raise ValueError("Invalid algorithm")

        self.data["algorithm"] = algorithms_type[AlgorithmType.TEMPERATURE][algorithm]


class SpeedController(Controllers):
    def __init__(
        self,
        algorithm: SpeedAlgorithmType = SpeedAlgorithmType.EASE_IN,
        speed: int = 0,
        direction: DirectionType = DirectionType.FORWARD,
    ):
        super().__init__()
        self.data = {
            "kind": controllers_type[ControllerType.SPEED],
            "algorithm": algorithms_type[AlgorithmType.SPEED][algorithm],
            "speed": speed,
            "direction": directions[direction],
        }

    def set_algorithm(self, algorithm: SpeedAlgorithmType):
        if algorithm not in algorithms_type[AlgorithmType.SPEED]:
            raise ValueError("Invalid algorithm")

        self.data["algorithm"] = algorithms_type[AlgorithmType.SPEED][algorithm]

    def set_speed(self, speed: int):
        self.data["speed"] = speed

    def set_direction(self, direction: DirectionType):
        if direction not in directions:
            raise ValueError("Invalid direction")

        self.data["direction"] = directions[direction]


class PowerController(CurveControllers):
    def __init__(
        self,
        algorithm: PowerAlgorithmType = PowerAlgorithmType.SPRING,
        curve_id: int = 0,
        interpolation_kind: CurveInterpolationType = CurveInterpolationType.LINEAR,
        points: list = [0, 8],
        reference_kind: ReferenceType = ReferenceType.TIME,
        reference_id: int = 0,
    ):
        super().__init__()
        self.data["kind"] = controllers_type[ControllerType.POWER]
        self.data["algorithm"] = algorithms_type[AlgorithmType.POWER][algorithm]
        self.data["curve"]["id"] = curve_id
        self.data["curve"]["interpolation_kind"] = curve_interpolation[interpolation_kind]
        self.data["curve"]["points"] = points
        self.data["curve"]["reference"]["kind"] = reference_type[ReferenceType.CURVE][
            reference_kind
        ]
        self.data["curve"]["reference"]["id"] = reference_id

    def set_algorithm(self, algorithm: PowerAlgorithmType):
        if algorithm not in algorithms_type[AlgorithmType.POWER]:
            raise ValueError("Invalid algorithm")

        self.data["algorithm"] = algorithms_type[AlgorithmType.POWER][algorithm]


class WeightController(CurveControllers):
    def __init__(
        self,
        algorithm: WeightAlgorithmType = WeightAlgorithmType.PID_V1,
        curve_id: int = 0,
        interpolation_kind: CurveInterpolationType = CurveInterpolationType.LINEAR,
        points: list = [0, 8],
        reference_kind: ReferenceType = ReferenceType.TIME,
        reference_id: int = 0,
    ):
        super().__init__()
        self.data["kind"] = controllers_type[ControllerType.WEIGHT]
        self.data["algorithm"] = algorithms_type[AlgorithmType.WEIGHT][algorithm]
        self.data["curve"]["id"] = curve_id
        self.data["curve"]["interpolation_kind"] = curve_interpolation[interpolation_kind]
        self.data["curve"]["points"] = points
        self.data["curve"]["reference"]["kind"] = reference_type[ReferenceType.CURVE][
            reference_kind
        ]
        self.data["curve"]["reference"]["id"] = reference_id

    def set_algorithm(self, algorithm: WeightAlgorithmType):
        if algorithm not in algorithms_type[AlgorithmType.WEIGHT]:
            raise ValueError("Invalid algorithm")

        self.data["algorithm"] = algorithms_type[AlgorithmType.WEIGHT][algorithm]


class LogController(Controllers):
    # This class displays a message
    def __init__(self, message: MessageType = MessageType.NO_WATER):
        super().__init__()
        self.data["kind"] = controllers_type[ControllerType.MESSAGE]
        self.data["message"] = messages[message]

    def set_message(self, message: MessageType):
        if message not in messages:
            raise ValueError("Invalid message")

        self.data["message"] = messages[message]


class TareController(Controllers):
    # This class is a controller that when called, the machine makes a tare
    def __init__(self):
        self.data = {"kind": controllers_type[ControllerType.TARE]}


class EndProfile(Controllers):
    # This class is a controller that when called, the machine finishes the profile
    def __init__(self):
        self.data = {"kind": controllers_type[ControllerType.END]}


class ReferenceController(Controllers):
    # This class is a controller that when called, the machine makes a reference
    def __init__(self):
        super().__init__()
        self.data["kind"] = reference_type[ReferenceType.CONTROL][ReferenceType.TIME]
        self.data["id"] = 0

    def set_reference_id(self, id: int):
        self.data["id"] = id


class TimeReferenceController(ReferenceController):
    # This class is a controller that when called, the machine makes a reference in time
    def __init__(self, id: int = 0):
        super().__init__()
        self.data["kind"] = reference_type[ReferenceType.CONTROL][ReferenceType.TIME]
        self.data["id"] = id

    def get_time_reference_id(self):
        return self.data["id"]


class PositionReferenceController(ReferenceController):
    # This class is a controller that when called, the machine makes a reference in position
    def __init__(self, id: int = 0):
        super().__init__()
        self.data["kind"] = reference_type[ReferenceType.CONTROL][ReferenceType.POSITION]
        self.data["id"] = id

    def get_position_reference_id(self):
        return self.data["id"]


class WeightReferenceController(ReferenceController):
    # This class is a controller that when called, the machine makes a reference in weight
    def __init__(self, id: int = 0):
        super().__init__()
        self.data["kind"] = reference_type[ReferenceType.CONTROL][ReferenceType.WEIGHT]
        self.data["id"] = id

    def get_weight_id(self):
        return self.data["id"]


if __name__ == "__main__":
    # Example usage of the Controllers class.

    # All the controllers are initialized with default values

    points = [[0, 6], [10, 8]]

    # First option to assign values to the control when it initializes
    pressure_controller_1 = PressureController(
        PressureAlgorithmType.PID_V1,
        7,
        CurveInterpolationType.LINEAR,
        points,
        ReferenceType.TIME,
        9,
    )
    # Second option to assign values to the control after it initializes
    # pressure_controller_1.set_algorithm(PressureAlgorithmType.PID_V1)
    # pressure_controller_1.set_curve_id(1)
    # pressure_controller_1.set_interpolation_kind(CurveInterpolationType.LINEAR)
    # pressure_controller_1.set_points(points)
    # pressure_controller_1.set_reference_type(ReferenceType.TIME)
    # pressure_controller_1.set_reference_id(2)
    print(json.dumps(pressure_controller_1.get_controller(), indent=4))

    #! The same process is repeated for the other controllers when the control accepts at least one parameter

    flow_controller_1 = FlowController(
        FlowAlgorithmType.PID_V1,
        1,
        CurveInterpolationType.LINEAR,
        points,
        ReferenceType.TIME,
        2,
    )
    # flow_controller_1.set_curve_id(3)
    # flow_controller_1.set_interpolation_kind(CurveInterpolationType.CATMULL)
    # flow_controller_1.set_points(points)
    # flow_controller_1.set_reference_type(ReferenceType.POSITION)
    # flow_controller_1.set_reference_id(4)
    print(json.dumps(flow_controller_1.get_controller(), indent=4))

    temperature_controller_1 = TemperatureController(
        TemperatureAlgorithmType.WATER,
        5,
        CurveInterpolationType.LINEAR,
        points,
        ReferenceType.WEIGHT,
        6,
    )
    # temperature_controller_1.set_algorithm(TemperatureAlgorithmType.WATER)
    # temperature_controller_1.set_curve_id(5)
    # temperature_controller_1.set_interpolation_kind(CurveInterpolationType.LINEAR)
    # temperature_controller_1.set_points(points)
    # temperature_controller_1.set_reference_type(ReferenceType.WEIGHT)
    # temperature_controller_1.set_reference_id(6)
    print(json.dumps(temperature_controller_1.get_controller(), indent=4))

    speed_controller_1 = SpeedController(SpeedAlgorithmType.EASE_IN, 7, DirectionType.FORWARD)
    # speed_controller_1.set_algorithm(SpeedAlgorithmType.EASE_IN)
    # speed_controller_1.set_speed(7)
    # speed_controller_1.set_direction(DirectionType.FORWARD)
    print(json.dumps(speed_controller_1.get_controller(), indent=4))

    power_controller_1 = PowerController(
        PowerAlgorithmType.SPRING,
        8,
        CurveInterpolationType.CATMULL,
        points,
        ReferenceType.TIME,
        9,
    )
    # power_controller_1.set_curve_id(7)
    # power_controller_1.set_interpolation_kind(CurveInterpolationType.CATMULL)
    # power_controller_1.set_points(points)
    # power_controller_1.set_reference_type(ReferenceType.TIME)
    # power_controller_1.set_reference_id(8)
    print(json.dumps(power_controller_1.get_controller(), indent=4))

    weight_controller_1 = WeightController(
        WeightAlgorithmType.PID_V1,
        9,
        CurveInterpolationType.LINEAR,
        points,
        ReferenceType.POSITION,
        10,
    )
    # weight_controller_1.set_curve_id(9)
    # weight_controller_1.set_interpolation_kind(CurveInterpolationType.LINEAR)
    # weight_controller_1.set_points(points)
    # weight_controller_1.set_reference_type(ReferenceType.POSITION)
    # weight_controller_1.set_reference_id(10)
    print(json.dumps(weight_controller_1.get_controller(), indent=4))

    log_controller_1 = LogController(MessageType.NO_WATER)
    # log_controller_1.set_message(MessageType.NO_WATER)
    print(json.dumps(log_controller_1.get_controller(), indent=4))

    tare_controller_1 = TareController()
    print(json.dumps(tare_controller_1.get_controller(), indent=4))

    end_profile_1 = EndProfile()
    print(json.dumps(end_profile_1.get_controller(), indent=4))

    time_reference_controller_1 = TimeReferenceController(100)
    # time_reference_controller_1.set_reference_id(11)
    print(json.dumps(time_reference_controller_1.get_controller(), indent=4))

    position_reference_controller_1 = PositionReferenceController(101)
    # position_reference_controller_1.set_reference_id(12)
    print(json.dumps(position_reference_controller_1.get_controller(), indent=4))

    weight_reference_controller_1 = WeightReferenceController(102)
    # weight_reference_controller_1.set_reference_id(13)
    print(json.dumps(weight_reference_controller_1.get_controller(), indent=4))

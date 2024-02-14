import json 
import dictionaries as dic
from dictionaries import controllers_type, algorithms_type, curve_interpolation, reference_type, directions, messages
class Controllers:
#This parent class has the get_controller method that returns a dictionary with the information of the controllers

    def __init__(self):
        self.data = {}
        
    def get_controller(self):
        return self.data

class CurveControllers(Controllers):
    
    '''
    This child class of Controllers is a special class made for controllers that have an associated curve.

    Attributes:

    set_curve_id: int -> ID of the curve associated with the controller
    set_interpolation_kind: str -> type of curve interpolation
    set_points: list -> list of points of the curve
    set_reference_type: str -> type of curve reference
    set_reference_id: int -> ID of the curve reference
    self.data: dict -> dictionary with the information of the controller and its associated curve
    ''' 
    def __init__(self):
        self.data = {
            "kind": "",
            "algorithm": "",
            "curve" : {
                "id" : 0,
                "interpolation_kind": "",
                "points" : [],
                "reference" :{
                    "kind" : "",
                    "id" : 0
                }
            }
        }
    
    def set_curve_id(self, id: int):
        self.data["curve"]["id"] = id

    def set_interpolation_kind(self, interpolation_kind: dic.enums.CurveInterpolationType):
        if interpolation_kind not in curve_interpolation:
            raise ValueError("Invalid interpolation kind")
        
        self.data["curve"]["interpolation_kind"] = curve_interpolation[interpolation_kind]
    
    def set_points(self, points: list):
        self.data["curve"]["points"] = points
        
    def set_reference_type(self, reference_kind: dic.enums.ReferenceType):
        if reference_kind not in reference_type[dic.enums.ReferenceType.CURVE]:
            raise ValueError("Invalid reference kind")
        self.data["curve"]["reference"]["kind"] = reference_type[dic.enums.ReferenceType.CURVE][reference_kind]
        
    def set_reference_id(self, reference_id: int):
        self.data["curve"]["reference"]["id"] = reference_id

'''
Child classes of CurveControllers that represent pressure, flow, temperature, power, and weight controllers
These classes have methods to change the controller's algorithm and to change the curve associated with the controller
'''
class PressureController(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type[dic.enums.ControllerType.PRESSURE]
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.PRESSURE][dic.enums.Pressure_Algorithm_Type.PID_V1]
        self.data["curve"]["id"] = 0
        self.data["curve"]["interpolation_kind"] = curve_interpolation[dic.enums.CurveInterpolationType.LINEAR]
        self.data["curve"]["points"] = [0,6]
        self.data["curve"]["reference"]["kind"] = reference_type[dic.enums.ReferenceType.CURVE][dic.enums.ReferenceType.TIME]
        self.data["curve"]["reference"]["id"] = 0
        
    
    def set_algorithm(self, algorithm: dic.enums.Pressure_Algorithm_Type):
    # only accept valid algorithms
        if algorithm not in algorithms_type[dic.enums.AlgorithmType.PRESSURE]:
            raise ValueError("Invalid algorithm")

        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.PRESSURE][algorithm]


class FlowController(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type[dic.enums.ControllerType.FLOW]
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.FLOW][dic.enums.Flow_Algorithm_Type.PID_V1]
        
    def set_algorithm(self, algorithm: dic.enums.Flow_Algorithm_Type):
        if algorithm not in algorithms_type[dic.enums.AlgorithmType.FLOW]:
            raise ValueError("Invalid algorithm")
        
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.FLOW][algorithm]
        
class TemperatureController(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type[dic.enums.ControllerType.TEMPERATURE]
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.TEMPERATURE][dic.enums.Temperature_Algorithm_Type.WATER]
        
    def set_algorithm(self, algorithm: dic.enums.Temperature_Algorithm_Type):
        if algorithm not in algorithms_type[dic.enums.AlgorithmType.TEMPERATURE]:
            raise ValueError("Invalid algorithm")
        
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.TEMPERATURE][algorithm]
        
class SpeedController(Controllers):
    def __init__(self):
        super().__init__()
        self.data = {
            "kind": controllers_type[dic.enums.ControllerType.SPEED],
            "algorithm": algorithms_type[dic.enums.AlgorithmType.SPEED][dic.enums.Speed_Algorithm_Type.EASE_IN],
            "speed" : 0,
            "direction" : ""
        }
        
    def set_algorithm(self, algorithm: dic.enums.Speed_Algorithm_Type):
        if algorithm not in algorithms_type[dic.enums.AlgorithmType.SPEED]:
            raise ValueError("Invalid algorithm")
        
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.SPEED][algorithm]
    
    def set_speed(self, speed: int):
        self.data["speed"] = speed
    
    def set_direction(self, direction: dic.enums.Direction_Type):
        if direction not in directions:
            raise ValueError("Invalid direction")
        
        self.data["direction"] = directions[direction]
        
class PowerController(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type[dic.enums.ControllerType.POWER]
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.POWER][dic.enums.Power_Algorithm_Type.SPRING]
    
    def set_algorithm(self, algorithm: dic.enums.Power_Algorithm_Type):
        if algorithm not in algorithms_type[dic.enums.AlgorithmType.POWER]:
            raise ValueError("Invalid algorithm")
        
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.POWER][algorithm]

class WeightController(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type[dic.enums.ControllerType.WEIGHT]
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.WEIGHT][dic.enums.Weight_Algorithm_Type.PID_V1]
        
    def set_algorithm(self, algorithm: dic.enums.Weight_Algorithm_Type):
        if algorithm not in algorithms_type[dic.enums.AlgorithmType.WEIGHT]:
            raise ValueError("Invalid algorithm")
        
        self.data["algorithm"] = algorithms_type[dic.enums.AlgorithmType.WEIGHT][algorithm]

class LogController(Controllers):
    #This class displays a message
    def __init__(self):
        self.data = {
            "kind": controllers_type[dic.enums.ControllerType.MESSAGE],
            "message": ""
        }
        
    def set_message(self, message: dic.enums.Message_Type):
        if message not in messages:
            raise ValueError("Invalid message")
        
        self.data["message"] = messages[message]

class TareController(Controllers):
    #This class is a controller that when called, the machine makes a tare
    def __init__(self):
        self.data = {
            "kind": controllers_type[dic.enums.ControllerType.TARE]
        }
    
class EndProfile(Controllers):
    #This class is a controller that when called, the machine finishes the profile
    def __init__(self):
        self.data = {
            "kind": controllers_type[dic.enums.ControllerType.END]
        }
        
class ReferenceController(Controllers):
    #This class is a controller that when called, the machine makes a reference
    def __init__(self):
        super().__init__()
        self.data["kind"] = reference_type[dic.enums.ReferenceType.CONTROL][dic.enums.ReferenceType.TIME]
        self.data["id"] = 0
        
    def set_reference_id(self, id: int):
        self.data["id"] = id
        
class TimeReferenceController(ReferenceController):
    #This class is a controller that when called, the machine makes a reference in time
    def __init__(self):
        super().__init__()
        self.data["kind"] = reference_type[dic.enums.ReferenceType.CONTROL][dic.enums.ReferenceType.TIME]
        
class PositionReferenceController(ReferenceController):
    #This class is a controller that when called, the machine makes a reference in position
    def __init__(self):
        super().__init__()
        self.data["kind"] = reference_type[dic.enums.ReferenceType.CONTROL][dic.enums.ReferenceType.POSITION]
        
class WeightReferenceController(ReferenceController):
    #This class is a controller that when called, the machine makes a reference in weight
    def __init__(self):
        super().__init__()
        self.data["kind"] = reference_type[dic.enums.ReferenceType.CONTROL][dic.enums.ReferenceType.WEIGHT]   
        
    
if __name__ == "__main__":
    # Example usage of the Controllers class.
    
    # All the controllers are initialized with default values    

    points = [[0, 6],[10,8]]
    
    pressure_controller_1 = PressureController()
    pressure_controller_1.set_algorithm(dic.enums.Pressure_Algorithm_Type.PID_V1)
    pressure_controller_1.set_curve_id(1)
    pressure_controller_1.set_interpolation_kind(dic.enums.CurveInterpolationType.LINEAR)
    pressure_controller_1.set_points(points)
    pressure_controller_1.set_reference_type(dic.enums.ReferenceType.TIME)
    pressure_controller_1.set_reference_id(2)
    print(json.dumps(pressure_controller_1.get_controller(), indent=4))
    
    flow_controller_1 = FlowController()
    flow_controller_1.set_curve_id(3)
    flow_controller_1.set_interpolation_kind(dic.enums.CurveInterpolationType.CATMULL)
    flow_controller_1.set_points(points)
    flow_controller_1.set_reference_type(dic.enums.ReferenceType.POSITION)
    flow_controller_1.set_reference_id(4)
    print(json.dumps(flow_controller_1.get_controller(), indent=4))
    
    temperature_controller_1 = TemperatureController()
    temperature_controller_1.set_algorithm(dic.enums.Temperature_Algorithm_Type.WATER)
    temperature_controller_1.set_curve_id(5)
    temperature_controller_1.set_interpolation_kind(dic.enums.CurveInterpolationType.LINEAR)
    temperature_controller_1.set_points(points)
    temperature_controller_1.set_reference_type(dic.enums.ReferenceType.WEIGHT)
    temperature_controller_1.set_reference_id(6)
    print(json.dumps(temperature_controller_1.get_controller(), indent=4))
    
    speed_controller_1 = SpeedController()
    speed_controller_1.set_algorithm(dic.enums.Speed_Algorithm_Type.EASE_IN)
    speed_controller_1.set_speed(7)
    speed_controller_1.set_direction(dic.enums.Direction_Type.FORWARD)
    print(json.dumps(speed_controller_1.get_controller(), indent=4))
    
    power_controller_1 = PowerController()
    power_controller_1.set_curve_id(7)
    power_controller_1.set_interpolation_kind(dic.enums.CurveInterpolationType.CATMULL)
    power_controller_1.set_points(points)
    power_controller_1.set_reference_type(dic.enums.ReferenceType.TIME)
    power_controller_1.set_reference_id(8)
    print(json.dumps(power_controller_1.get_controller(), indent=4))
    
    weight_controller_1 = WeightController()
    weight_controller_1.set_curve_id(9)
    weight_controller_1.set_interpolation_kind(dic.enums.CurveInterpolationType.LINEAR)
    weight_controller_1.set_points(points)
    weight_controller_1.set_reference_type(dic.enums.ReferenceType.POSITION)
    weight_controller_1.set_reference_id(10)
    print(json.dumps(weight_controller_1.get_controller(), indent=4))
    
    log_controller_1 = LogController()
    log_controller_1.set_message(dic.enums.Message_Type.NO_WATER) 
    print(json.dumps(log_controller_1.get_controller(), indent=4))
    
    tare_controller_1 = TareController()
    print(json.dumps(tare_controller_1.get_controller(), indent=4))
    
    end_profile_1 = EndProfile()
    print(json.dumps(end_profile_1.get_controller(), indent=4))
    
    time_reference_controller_1 = TimeReferenceController()
    time_reference_controller_1.set_reference_id(11)
    print(json.dumps(time_reference_controller_1.get_controller(), indent=4))
    
    position_reference_controller_1 = PositionReferenceController()
    position_reference_controller_1.set_reference_id(12)
    print(json.dumps(position_reference_controller_1.get_controller(), indent=4))
    
    weight_reference_controller_1 = WeightReferenceController()
    weight_reference_controller_1.set_reference_id(13)
    print(json.dumps(weight_reference_controller_1.get_controller(), indent=4))
    
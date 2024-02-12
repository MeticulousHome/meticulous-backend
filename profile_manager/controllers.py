import json 
from dictionaries import controllers_type, algorithms_type, reference_type, curve_interpolation, messages, directions
from dictionaries import ReferenceType, Pressure_Algorithm_Type, Temperature_Algorithm_Type, Speed_Algorithm_Type, Message_Type, CurveInterpolationType, Direction_Type

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

    def set_interpolation_kind(self, interpolation_kind: str):
        if interpolation_kind not in curve_interpolation:
            raise ValueError("Invalid interpolation kind")
        
        self.data["curve"]["interpolation_kind"] = curve_interpolation[interpolation_kind]
    
    def set_points(self, points: list):
        self.data["curve"]["points"] = points
        
    def set_reference_type(self, reference_kind: ReferenceType):
        if reference_kind not in reference_type["curve"]:
            raise ValueError("Invalid reference kind")
        self.data["curve"]["reference"]["kind"] = reference_type["curve"][reference_kind]
        
    def set_reference_id(self, reference_id: int):
        self.data["curve"]["reference"]["id"] = reference_id

'''
Child classes of CurveControllers that represent pressure, flow, temperature, power, and weight controllers
These classes have methods to change the controller's algorithm and to change the curve associated with the controller
'''
class pressure_controller(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type["pressure"]
        self.data["algorithm"] = algorithms_type["pressure"][Pressure_Algorithm_Type.PID_V1]
        self.data["curve"]["id"] = 0
        self.data["curve"]["interpolation_kind"] = curve_interpolation[CurveInterpolationType.LINEAR]
        self.data["curve"]["points"] = [0,6]
        self.data["curve"]["reference"]["kind"] = reference_type["curve"][ReferenceType.TIME]
        self.data["curve"]["reference"]["id"] = 0
        
    
    def set_algorithm(self, algorithm: Pressure_Algorithm_Type):
    # only accept valid algorithms
        if algorithm not in algorithms_type["pressure"]:
            raise ValueError("Invalid algorithm")

        self.data["algorithm"] = algorithms_type["pressure"][algorithm]


class flow_controller(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type["flow"]
        self.data["algorithm"] = algorithms_type["flow"]
        
class temperature_controller(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type["temperature"]
        self.data["algorithm"] = algorithms_type["temperature"][Temperature_Algorithm_Type.WATER]
        
    def set_algorithm(self, algorithm: Temperature_Algorithm_Type):
        if algorithm not in algorithms_type["temperature"]:
            raise ValueError("Invalid algorithm")
        
        self.data["algorithm"] = algorithms_type["temperature"][algorithm]
        
class speed_controller(Controllers):
    def __init__(self):
        super().__init__()
        self.data = {
            "kind": controllers_type["speed"],
            "algorithm": algorithms_type["speed"][Speed_Algorithm_Type.EASE_IN],
            "speed" : 0,
            "direction" : ""
        }
        
    def set_algorithm(self, algorithm: Speed_Algorithm_Type):
        if algorithm not in algorithms_type["speed"]:
            raise ValueError("Invalid algorithm")
        
        self.data["algorithm"] = algorithms_type["speed"][algorithm]
    
    def set_speed(self, speed: int):
        self.data["speed"] = speed
    
    def set_direction(self, direction: Direction_Type):
        if direction not in directions:
            raise ValueError("Invalid direction")
        
        self.data["direction"] = directions[direction]
        
class power_controller(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type["power"]
        self.data["algorithm"] = algorithms_type["power"]

class weight_controller(CurveControllers):
    def __init__(self):
        super().__init__()
        self.data["kind"] = controllers_type["weight"]
        self.data["algorithm"] = algorithms_type["weight"]

class log_controller(Controllers):
    #This class displays a message
    def __init__(self):
        self.data = {
            "kind": controllers_type["message"],
            "message": ""
        }
        
    def set_message(self, message: Message_Type):
        if message not in messages:
            raise ValueError("Invalid message")
        
        self.data["message"] = messages[message]

class tare_controller(Controllers):
    #This class is a controller that when called, the machine makes a tare
    def __init__(self):
        self.data = {
            "kind": controllers_type["tare"]
        }
    
class end_profile(Controllers):
    #This class is a controller that when called, the machine finishes the profile
    def __init__(self):
        self.data = {
            "kind": controllers_type["end"]
        }
        
    
if __name__ == "__main__":
    # Example usage of the Controllers class.
    
    # All the controllers are initialized with default values    

    points = [[0, 6],[10,8]]
    
    pressure_controller_1 = pressure_controller()
    pressure_controller_1.set_algorithm(Pressure_Algorithm_Type.PID_V1)
    pressure_controller_1.set_curve_id(1)
    pressure_controller_1.set_interpolation_kind(CurveInterpolationType.LINEAR)
    pressure_controller_1.set_points(points)
    pressure_controller_1.set_reference_type(ReferenceType.TIME)
    pressure_controller_1.set_reference_id(2)
    print(json.dumps(pressure_controller_1.get_controller(), indent=4))
    
    flow_controller_1 = flow_controller()
    flow_controller_1.set_curve_id(3)
    flow_controller_1.set_interpolation_kind(CurveInterpolationType.CATMULL)
    flow_controller_1.set_points(points)
    flow_controller_1.set_reference_type(ReferenceType.POSITION)
    flow_controller_1.set_reference_id(4)
    print(json.dumps(flow_controller_1.get_controller(), indent=4))
    
    temperature_controller_1 = temperature_controller()
    temperature_controller_1.set_algorithm(Temperature_Algorithm_Type.WATER)
    temperature_controller_1.set_curve_id(5)
    temperature_controller_1.set_interpolation_kind(CurveInterpolationType.LINEAR)
    temperature_controller_1.set_points(points)
    temperature_controller_1.set_reference_type(ReferenceType.WEIGHT)
    temperature_controller_1.set_reference_id(6)
    print(json.dumps(temperature_controller_1.get_controller(), indent=4))
    
    speed_controller_1 = speed_controller()
    speed_controller_1.set_algorithm(Speed_Algorithm_Type.EASE_IN)
    speed_controller_1.set_speed(7)
    speed_controller_1.set_direction(Direction_Type.FORWARD)
    print(json.dumps(speed_controller_1.get_controller(), indent=4))
    
    power_controller_1 = power_controller()
    power_controller_1.set_curve_id(7)
    power_controller_1.set_interpolation_kind(CurveInterpolationType.CATMULL)
    power_controller_1.set_points(points)
    power_controller_1.set_reference_type(ReferenceType.TIME)
    power_controller_1.set_reference_id(8)
    print(json.dumps(power_controller_1.get_controller(), indent=4))
    
    weight_controller_1 = weight_controller()
    weight_controller_1.set_curve_id(9)
    weight_controller_1.set_interpolation_kind(CurveInterpolationType.LINEAR)
    weight_controller_1.set_points(points)
    weight_controller_1.set_reference_type(ReferenceType.POSITION)
    weight_controller_1.set_reference_id(10)
    print(json.dumps(weight_controller_1.get_controller(), indent=4))
    
    log_controller_1 = log_controller()
    log_controller_1.set_message(Message_Type.NO_WATER) 
    print(json.dumps(log_controller_1.get_controller(), indent=4))
    
    tare_controller_1 = tare_controller()
    print(json.dumps(tare_controller_1.get_controller(), indent=4))
    
    end_profile_1 = end_profile()
    print(json.dumps(end_profile_1.get_controller(), indent=4))
    
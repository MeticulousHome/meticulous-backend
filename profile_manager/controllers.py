import json 
from dictionaries import controllers_type, algorithms_type, reference_type, curve_interpolation
from dictionaries import ReferenceType, Pressure_Algorithm_Type, Temperature_Algorithm_Type, Speed_Algorithm_Type
class Controllers:
    def __init__(self):
        self.data = {}
        
    def get_controller(self):
        return self.data

class CurveControllers(Controllers):
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
        self.algorithm_dictionary = []
        self.reference_diccionary = []
    
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

class pressure_controller(CurveControllers):
    def __init__(self):
        self.data["kind"] = controllers_type["pressure"]
        self.data["algorithm"] = algorithms_type["pressure"]["pid v1"]
        self.data["curve"]["id"] = 0
        self.data["curve"]["interpolation_kind"] = curve_interpolation["linear"]
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
        self.data["kind"] = controllers_type["flow"]
        self.data["algorithm"] = algorithms_type["flow"]
        
class temperature_controller(CurveControllers):
    def __init__(self):
        self.data["kind"] = controllers_type["temperature"]
        self.data["algorithm"] = algorithms_type["temperature"]["water"]
        
    def set_algorithm(self, algorithm: Temperature_Algorithm_Type):
        if algorithm not in algorithms_type["temperature"]:
            raise ValueError("Invalid algorithm")
        
        self.data["algorithm"] = algorithms_type["temperature"][algorithm]
        
class speed_controller(Controllers):
    def __init__(self):
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
        
    
    
    
    def pressure_controller(self, curve_id: int, interpolation_kind: str, points: list, reference_kind: str, reference_id: int):
        self.data = {
            "kind": self.controllers_type["pressure"],
            "algorithm": self.algorithms_type["pressure"]["pid v1"],
            "curve" : {
                "id" : curve_id,
                "interpolation_kind": self.curve_interpolation[interpolation_kind],
                "points" : points,
                "reference" :{
                    "kind" : self.reference_type["kind"][reference_kind],
                    "id" : reference_id
                }
            }
        }
        return self.data
    
    def flow_controller(self, curve_id: int, interpolation_kind: str, points: list, reference_kind: str, reference_id: int):
        self.data = {
            "kind": self.controllers_type["flow"],
            "algorithm": self.algorithms_type["flow"],
            "curve" : {
                "id" : curve_id,
                "interpolation_kind" : self.curve_interpolation[interpolation_kind],
                "points" : points,
                "reference" :{
                    "kind" : self.reference_type["kind"][reference_kind],
                    "id" : reference_id
                }
            }
        }
        return self.data
        
    def power_controller(self, curve_id: int, interpolation_kind: str, points: list, reference_kind: str, reference_id: int):
        self.data = {
            "kind": self.controllers_type["power"],
            "algorithm": self.algorithms_type["power"],
            "curve" : {
                "id" : curve_id,
                "interpolation_kind" : self.curve_interpolation[interpolation_kind],
                "points" : points,
                "reference" :{
                    "kind" : self.reference_type["kind"][reference_kind],
                    "id" : reference_id
                }
            }
        }
        return self.data
    
    def weight_controller(self, curve_id: int, interpolation_kind: str, points: list, reference_kind: str, reference_id: int):
        self.data = {
            "kind": self.controllers_type["weight"],
            "algorithm": self.algorithms_type["weight"],
            "curve" : {
                "id" : curve_id,
                "interpolation_kind" : self.curve_interpolation[interpolation_kind],
                "points" : points,
                "reference" :{
                    "kind" : self.reference_type["kind"][reference_kind],
                    "id" : reference_id
                }
            }
        }
        return self.data
    
    def temperature_controller(self, curve_id: int, interpolation_kind: str, points: list, reference_kind: str, reference_id: int, algorithm: str):
        self.data = {
            "kind": self.controllers_type["temperature"],
            "algorithm": self.algorithms_type["temperature"][algorithm],
            "curve" : {
                "id" : curve_id,
                "interpolation_kind" : self.curve_interpolation[interpolation_kind],
                "points" : points,
                "reference" :{
                    "kind" : self.reference_type["kind"][reference_kind],
                    "id" : reference_id
                }
            }
        }
        return self.data
    
    def speed_controller(self, speed_value: int, algorithm: str, direction: str):
        self.data = {
            "kind": self.controllers_type["speed"],
            "algorithm": self.algorithms_type["speed"][algorithm],
            "speed" : speed_value,
            "direction" : self.direction[direction]
        }
        return self.data
    
    def log_controller(self, message: str):
        self.data = {
            "kind": self.controllers_type["message"],
            "message": self.messages[message]
        }
        return self.data
    
    def tare_controller(self):
        self.data = {
            "kind": self.controllers_type["tare"]
        }
        return self.data
    
    def end_profile(self):
        self.data = {
            "kind": self.controllers_type["end"]
        }
        return self.data
    
    def build_reference(self, id: int, reference_kind: str):
        self.data = {
            "kind": self.reference_type["reference"][reference_kind],
            "id": id
        }
        return self.data
    
    
    
    
    
if __name__ == "__main__":
    # Example usage of the Controllers class.
    
    """
    Controllers Class Documentation:

    The Controllers class is a powerful tool for converting simple JSON into more complex JSON structures, 
    specifically designed for handling various types of controls within the application. 
    Here's a breakdown of what this class offers and how to use it effectively:

    Key Features:
    - This class is your go-to for creating controls such as pressure, flow, power, weight, and temperature. 
    Each control type is supported by a set of key features including:
        Algorithm, Curve ID, Interpolation kind, Points, Reference type, and Reference ID.
    - For each type of control, there's a dedicated method in this class. These methods are designed to take arguments like:
    curve ID, interpolation kind, points, reference kind, and reference ID. 
    - The temperature control stands out because it requires an additional algorithm argument.

    Handling References:
    - In the realm of complex JSON, references are treated uniquely. 
    The `build_reference` method in this class allows to build these references by specifying the reference ID and the type
    it belongs to. Currently, this includes types such as Time, Position, and Weight.

    Special Controls:
    - The class also introduces special controls for specific needs:
      - Speed control combines control type, algorithm, and specifics like speed and direction.
      - Tare control simplifies the process of calling the "tare" function on a scale.
      - The "end_profile" control signals the completion of a profile.
      - Lastly, the "log" control is designed for message logging, ensuring you can easily display messages as needed.

    """

    controllers = Controllers()
    points = [
        [0, 1],
        [10, 2],
        [30, 3]
    ]
    print(json.dumps(controllers.pressure_controller(1, "linear", points, "time", 10), indent=4))
    print(json.dumps(controllers.flow_controller(1, "linear", points, "time", 10), indent=4))
    print(json.dumps(controllers.power_controller(1, "linear", points, "time", 10), indent=4))
    print(json.dumps(controllers.weight_controller(1, "linear", points, "time", 10), indent=4))
    print(json.dumps(controllers.temperature_controller(1, "linear", points, "time", 10, "water"), indent=4))
    print(json.dumps(controllers.speed_controller(10, "ease-in", "forward"), indent=4))
    print(json.dumps(controllers.log_controller("no water"), indent=4))
    print(json.dumps(controllers.tare_controller(), indent=4))
    print(json.dumps(controllers.end_profile(), indent=4))
    print(json.dumps(controllers.build_reference(1, "time"), indent=4))
    print(json.dumps(controllers.build_reference(1, "weight"), indent=4))
    print(json.dumps(controllers.build_reference(1, "position"), indent=4))
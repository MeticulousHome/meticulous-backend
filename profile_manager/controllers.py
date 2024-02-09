import json 

class Controllers:
    def __init__(self):
        self.data = {}
        self.controllers_type = {
            "power": "piston_power_controller",
            "flow" : "flow_controller",
            "pressure" : "pressure_controller",
            "weight" : "weight_controller",
            "speed" : "move_piston_controller",
            "temperature" : "temperature_controller",
            "tare" : "tare_controller",
            "message" : "log_controller",
            "end" : "end_profile",
            "weight" : "weight_controller",
            "end" : "end_profile"
        }
        
        self.algorithms_type = {
            "pressure" : {
                "pid v1" : "Pressure PID v1.0",
                "pid v2" : "Pressure PID v2.0",
            },
            "power" : "Spring v1.0",
            "temperature" : {
                "water" : "Water Temperature PID v1.0",
                "cylinder" : "Cylinder Temperature PID v1.0",
                "tube" : "Tube Temperature PID v1.0",
                "plunger" : "Plunger Temperature PID v1.0",
                "stable" : "Stable Temperature"
            },
            "flow" : "Flow PID v1.0",
            "weight" : "Weight PID v1.0",
            "speed" : {
                "ease-in" : "Piston Ease-In",
                "fast" : "Piston Fast",
            }
        } 
        
        self.reference_type = {
            "kind" : {
                "time" : "time",
                "position" : "position",
                "weight" : "weight"
            },
            "reference" : {
                "time" : "time_reference",
                "position" : "position_reference",
                "weight" : "weight_reference"
            }
        }
        
        self.curve_interpolation = {
            "linear" : "linear_interpolation",
            "catmull" : "catmull_interpolation"
        }
        
        self.messages = {
            "no water" : "No Water",
            "remove cup" : "Remove Cup",
            "purge" : "Purge",
            "start click" : "Click to start",
            "purge click" : "Click to purge"
        }
        
        self.direction = {
            "forward" : "DOWN",
            "backward" : "UP"
        }


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
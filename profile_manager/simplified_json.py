import json
from dotenv import load_dotenv
from stages import *


class SimplifiedJson:
    
    """
    A simplified JSON class that allows loading, displaying,
    and converting JSON data to a complex JSON.

    Attributes:
        parameters (dict): A dictionary to store JSON data.
    """
    
    def __init__(self, parameters: json):
        self.parameters = {}
        if parameters is not None:
            self.parameters = parameters
        self.parameters["temperature"] = 80
    
    def load_simplified_json(self, parameters: json):
        self.parameters = parameters
        return self.parameters
    
    def show_simplified_json(self):
        self.parameters = json.dumps(self.parameters, indent=2)
        return self.parameters
    
    def get_temperature(self):
        return self.parameters["temperature"]
    
    
    def to_complex(self):
        """
        A placeholder method for converting JSON data to a complex JSON format.
        Currently, it is not implemented.

        Returns:
            str: A message indicating the method is not implemented.
        """
        return "Not implemented yet"
    
class InitNode(Nodes):
    def __init__(self, id : int = -1, time_ref_id: int = 1, weight_ref_id: int = 2, position_ref_id : int = 3, next_node_id: int = -1):
        self.node = Nodes(id)
        if time_ref_id is not None:
            print("Time ref id is not None")
            self.time_reference = TimeReferenceController()
            self.set_time_id(time_ref_id)
        if weight_ref_id is not None:
            self.weight_reference = WeightReferenceController()
            self.set_weight_id(weight_ref_id)
        if position_ref_id is not None:
            self.position_reference = PositionReferenceController()
            self.set_position_id(position_ref_id)
        self.exit_trigger = ExitTrigger(next_node_id)
        self.node.add_controller(self.time_reference)
        self.node.add_controller(self.weight_reference)
        self.node.add_controller(self.position_reference)
        self.node.add_trigger(self.exit_trigger)
        
    def set_time_id(self, id: int):
        self.time_reference.set_reference_id(id)
        
    def set_weight_id(self, id: int):
        self.weight_reference.set_reference_id(id)
        
    def set_position_id(self, id: int):
        self.position_reference.set_reference_id(id)
        
    def set_next_node_id(self, id: int):
        self.exit_trigger.set_next_node_id(id)
        
    def get_init_node(self):
        return self.node.get_node()           



    
if __name__ == "__main__":
    # Example usage of the SimplifiedJson class.
    
    file_path = "simplified_json_example.json"
    with open(file_path, 'r') as file:
        data = json.load(file)
    
    simplified_json = SimplifiedJson(data)
    simplified_json.load_simplified_json(data) # Another way to load the JSON data.
    # print(simplified_json.show_simplified_json())
    print(simplified_json.get_temperature())

    # Example of initializing a node with references to time, weight, and position.
    init_node = InitNode(-1, 1, 2, 3, 15)
    # print(json.dumps(init_node.get_init_node(), indent=2)) # Uncomment to see the example.
    
    # Example of setting the time, weight, and position reference IDs.
    init_node_1 = InitNode(-1)
    init_node_1.set_time_id(15)
    init_node_1.set_weight_id(16)
    init_node_1.set_position_id(17)
    init_node_1.set_next_node_id(18)
    print(json.dumps(init_node_1.get_init_node(), indent=2)) # Uncomment to see the example.
import json
from stages import *
from dictionaries_simplified import *
current_node_id = 1
class SimplifiedJson:
    
    """
    A simplified JSON class that allows loading, displaying,
    and converting JSON data to a complex JSON.

    Attributes:
        parameters (dict): A dictionary to store JSON data.
    """
    
    def __init__(self, parameters : dict = None):
        self.parameters = parameters if parameters is not None else {}
    
    def load_simplified_json(self, parameters):
        self.parameters = parameters
        return self.parameters
    
    def show_simplified_json(self):
        self.parameters = json.dumps(self.parameters, indent=2)
        return self.parameters
    
    def get_temperature(self):
        return self.parameters["temperature"]
    
    def get_new_node_id(self):
        global current_node_id
        current_node_id += 1
        return current_node_id - 1
    
    def to_complex(self):
        global current_node_id

        # Use the comments with * as debugging tools.
        complex_stages = []
        i = 0
        for stage_index, stage in enumerate(self.parameters.get("stages")):
                   
            init_node = Nodes(self.get_new_node_id()) 
            main_node = Nodes(self.get_new_node_id())
            
            init_note_time_reference = TimeReferenceController(1)
            init_note_weight_reference = WeightReferenceController(2)
            init_note_position_reference = PositionReferenceController(3)
            init_node_exit_trigger = ExitTrigger(main_node.get_node_id())

            init_node.add_controller(init_note_time_reference)
            init_node.add_controller(init_note_weight_reference)
            init_node.add_controller(init_note_position_reference)
            init_node.add_trigger(init_node_exit_trigger) 
            
            
            stage_name = stage.get("name")
            
            exit_triggers = []
            limit_triggers = []
            all_nodes = [init_node.get_node(), main_node.get_node()]
            
            for limit in stage["limits"]:
                limit_node = None
                limit_trigger = None
                
                match limit["type"]:
                    case "pressure":
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [0, trigger_limit_value]
                        limit_controller = PressureController(Pressure_Algorithm_Type.PID_V1, 7, CurveInterpolationType.LINEAR, points_trigger, ReferenceType.TIME, 9)
                        limit_id = limit_node.get_node_id()
                        limit_trigger = PressureValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, trigger_limit_value, limit_id)
                        limit_node.add_controller(limit_controller) 
                        
                    case "flow":
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [0, trigger_limit_value]
                        limit_controller = FlowController(Flow_Algorithm_Type.PID_V1, 1, CurveInterpolationType.LINEAR, points_trigger, ReferenceType.TIME, 9)
                        limit_id = limit_node.get_node_id()
                        limit_trigger = FlowValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, trigger_limit_value, limit_id)
                        limit_node.add_controller(limit_controller) 
                        
                    case "temperature":
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [0, trigger_limit_value]
                        limit_controller = TemperatureController(Temperature_Algorithm_Type.WATER, 4, CurveInterpolationType.LINEAR, points_trigger, ReferenceType.TIME, 9)
                        limit_id = limit_node.get_node_id()
                        limit_trigger = TemperatureValueTrigger(TemperatureSourceType.WATER, TriggerOperatorType.GREATER_THAN_OR_EQUAL, trigger_limit_value, limit_id)
                        limit_node.add_controller(limit_controller) 
                        
                    case "power":
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [0, trigger_limit_value]
                        limit_controller = PowerController(Power_Algorithm_Type.SPRING, 1,  CurveInterpolationType.LINEAR, points_trigger, ReferenceType.TIME, 9)
                        limit_id = limit_node.get_node_id()
                        limit_trigger = PowerValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, trigger_limit_value, limit_id)
                        limit_node.add_controller(limit_controller) 
    
                    case _:
                        print(f"Limit type: {limit['type']} not found.")
                all_nodes.append(limit_node.get_node())
                limit_triggers.append(limit_trigger.get_trigger())
                
            next_stage_node_id = self.get_new_node_id()
            current_node_id = next_stage_node_id
            
            for exits in stage["exit_triggers"]:
                match exits["type"]:
                    case "time":
                        exit_trigger_value = exits["value"] 
                        if exits["relative"]:
                            reference_id = 2
                        else:
                            reference_id = 0
                        exit_trigger = TimerTrigger(TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, reference_id, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        print(F"Next Stage Node ID after match: {next_stage_node_id} from the stage {stage_name}")
    
                    case "weight":
                        exit_trigger_value = exits["value"]
                        if exits["relative"]:
                            reference_id = 3
                        else:
                            reference_id = 100
                        exit_trigger = WeightTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, reference_id, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        
                    case "pressure":
                        exit_trigger_value = exits["value"]
                        exit_trigger = PressureValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                    
                    case "flow":    
                        exit_trigger_value = exits["value"]
                        exit_trigger = FlowValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        
                    case "piston_position":
                        exit_trigger_value = exits["value"]
                        if exits["relative"]:
                            reference_id = 1
                        else:
                            reference_id = 0
                        exit_trigger = PistonPositionTrigger(TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, reference_id, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        
                    case "power":
                        exit_trigger_value = exits["value"]
                        exit_trigger = PowerValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        
                    case "temperature":
                        exit_trigger_value = exits["value"]
                        exit_trigger = TemperatureValueTrigger(TemperatureSourceType.WATER, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                    case _:
                        print(f"Exit type: {exits['type']} not found.")
            for limit_node in all_nodes[2:]:
                limit_node["triggers"] += [trigger for trigger in limit_triggers if trigger["next_node_id"] != limit_node["id"]]
                limit_node["triggers"] += exit_triggers
                
                # limit_node_id = limit_node["id"] # *Get the limit node id from the limit node of the stages
                # trigger_next_node_id = limit_node["triggers"][0]["next_node_id"] # *Get the next node id from the exit triggers of the stages
            
            for trigger in exit_triggers:
                trigger = Triggers(trigger)
                main_node.add_trigger(trigger)
            for trigger in limit_triggers:
                trigger = Triggers(trigger)
                main_node.add_trigger(trigger)
                
            dynamics = stage.get("dynamics")
            type_main_controller = stage.get("type")
            points_main_controller = dynamics.get("points")
            over_main_controller = dynamics.get("over")
            interpolation_main_controller = dynamics.get("interpolation")
            main_node_id = main_node.get_node_id()
            
            match type_main_controller:
                case "pressure":
                    main_controller = PressureController(Pressure_Algorithm_Type.PID_V1, 7, interpolation_dict[interpolation_main_controller], points_main_controller, over_dict[over_main_controller], 9)
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = PressureCurveTrigger(SourceType.RAW,TriggerOperatorType.GREATER_THAN_OR_EQUAL, main_curve_id, main_node_id)
                    main_trigger = main_trigger.get_trigger()
                    
                case "flow":
                    main_controller = FlowController(Flow_Algorithm_Type.PID_V1, 8, interpolation_dict[interpolation_main_controller], points_main_controller, over_dict[over_main_controller], 9)
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = FlowCurveTrigger(SourceType.RAW,TriggerOperatorType.GREATER_THAN_OR_EQUAL, main_curve_id, main_node_id)
                    main_trigger = main_trigger.get_trigger()
                    
                case "temperature":
                    main_controller = TemperatureController(Temperature_Algorithm_Type.WATER, 4, interpolation_dict[interpolation_main_controller], points_main_controller, over_dict[over_main_controller], 9)
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = TemperatureCurveTrigger(SourceType.RAW,TriggerOperatorType.GREATER_THAN_OR_EQUAL, main_curve_id, 4)
                    main_trigger = main_trigger.get_trigger()
                    
                case "power":
                    main_controller = PowerController(Power_Algorithm_Type.SPRING, 1, interpolation_dict[interpolation_main_controller], points_main_controller, over_dict[over_main_controller], 9)
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = PowerCurveTrigger(SourceType.RAW,TriggerOperatorType.GREATER_THAN_OR_EQUAL, main_curve_id, main_node_id)
                    main_trigger = main_trigger.get_trigger()
                    
                case _:
                    print(f"Type: {type_main_controller} not found.")
                    
            
            
            for limit_node in all_nodes[2:]:
                limit_node["triggers"].append(main_trigger)     
                       

            complex_stages.append({
                "name": f"{stage_name}",
                "nodes": all_nodes
            })
                        
            print(f"Complex stage nodes from the {stage_name}:") # *Print the complex stages with the stage's name
            print(json.dumps(complex_stages, indent=2)) # *Print the complex stages with json format to see the changes.
            i = i+1
        return complex_stages
    
class InitNode(Nodes):
    def __init__(self, id : int = -1, time_ref_id: int = 1, weight_ref_id: int = 2, position_ref_id : int = 3, next_node_id: int = -1):
        super().__init__()
        self.set_id(id)
        if time_ref_id is not None:
            self.time_reference = TimeReferenceController()
            self.set_time_id(time_ref_id)
        if weight_ref_id is not None:
            self.weight_reference = WeightReferenceController()
            self.set_weight_id(weight_ref_id)
        if position_ref_id is not None:
            self.position_reference = PositionReferenceController()
            self.set_position_id(position_ref_id)
        self.exit_trigger = ExitTrigger(next_node_id)
        self.add_controller(self.time_reference)
        self.add_controller(self.weight_reference)
        self.add_controller(self.position_reference)
        self.add_trigger(self.exit_trigger)
        
    def set_time_id(self, id: int):
        self.time_reference.set_reference_id(id)
        
    def set_weight_id(self, id: int):
        self.weight_reference.set_reference_id(id)
        
    def set_position_id(self, id: int):
        self.position_reference.set_reference_id(id)
        
    def set_next_node_id(self, id: int):
        self.exit_trigger.set_next_node_id(id)      
    
    


    
if __name__ == "__main__":
    # Example usage of the SimplifiedJson class.
    
    file_path = "simplified_json_example.json"
    with open(file_path, 'r') as file:
        data = json.load(file)
    
    simplified_json = SimplifiedJson(data)
    simplified_json.load_simplified_json(data) # Another way to load the JSON data.
    # print(simplified_json.show_simplified_json())
    # print(simplified_json.get_temperature())
    # print(json.dumps(simplified_json.to_complex(), indent=2))   
    # print(simplified_json.main_node(1))
    
    complex_node = simplified_json.to_complex()
    
    
    points = [[0, 6],[10,8]]
    trigger = WeightTrigger(SourceType.AVERAGE, TriggerOperatorType.GREATER_THAN, 10, 12)

    # print(f"Node ID: {main_node.get_node_id()}")
    # print(json.dumps(main_node.get_node(), indent=2))
    
    # print(json.dumps(main_node.get_node(), indent=2)) # Uncomment to see the example.

    # Example of initializing a node with references to time, weight, and position.
    init_node = InitNode(-1)
    # print(json.dumps(init_node.get_init_node(), indent=2)) # Uncomment to see the example.
    
    # Example of setting the time, weight, and position reference IDs.
    init_node_1 = InitNode(-100)
    init_node_1.set_time_id(15)
    init_node_1.set_weight_id(16)
    init_node_1.set_position_id(17)
    init_node_1.set_next_node_id(18)
    # print(init_node_1["triggers"]) 

    # print(json.dumps(init_node_1.get_node(), indent=2)) # Uncomment to see the example.
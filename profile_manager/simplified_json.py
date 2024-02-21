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
        for stage_index, stage in enumerate(self.parameters.get("stages")):
            # print(stage) # *Print the stages in the JSON file.
            # print(stage_index) # *Print the index of the stages in the JSON file.
            print(f"beginning of the stage number {stage_index + 1}") # *Print the beginning of the loop        
            init_node = Nodes(self.get_new_node_id()) 
            print(f"Init Node ID: {init_node.get_node_id()}") # *Print the ID of the init node.
            main_node = Nodes(self.get_new_node_id())
            print(f"Main Node ID: {main_node.get_node_id()}") # *Print the ID of the main node.
            
            init_note_time_reference = TimeReferenceController(1)
            init_note_weight_reference = WeightReferenceController(2)
            init_note_position_reference = PositionReferenceController(3)
            print(f"Main node before assign to the exit trigger {main_node.get_node_id}")
            init_node_exit_trigger = ExitTrigger(main_node.get_node_id())
            print(f"Main node after assign to the exit trigger {main_node.get_node_id}")
            init_node.add_controller(init_note_time_reference)
            init_node.add_controller(init_note_weight_reference)
            init_node.add_controller(init_note_position_reference)
            init_node.add_trigger(init_node_exit_trigger) 
            
            
            # init_node.set_next_node_id(main_node.get_node_id())
            print(f"Next Node ID: {main_node.get_node_id()} in the iteration {stage_index + 1} for the init node") # *
            
            
            print(json.dumps(init_node.get_node(), indent=2)) # *Print the init node with json format to see the changes.
            
            stage_name = stage.get("name")
            
            exit_triggers = []
            limit_triggers = []
            all_nodes = [init_node.get_node(), main_node.get_node()]
            
            for limit in stage["limits"]:
                limit_node = None
                limit_trigger = None
                # print(f"Stage: {stage_name}") # *Print the stage name from the stage in the JSON file to math the name with its limits
                # print(json.dumps(limit, indent=2)) # *Print the limits from the stage in the JSON file
                match limit["type"]:
                    case "pressure":
                        # print(f"limit type in {stage_name} is {limit['type']}") # *Print the limit type from the stage in the JSON file according to the limits in the stages
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [0, trigger_limit_value]
                        limit_controller = PressureController(Pressure_Algorithm_Type.PID_V1, 7, CurveInterpolationType.LINEAR, points_trigger, ReferenceType.TIME, 9)
                        limit_id = limit_node.get_node_id()
                        limit_trigger = PressureValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, trigger_limit_value, limit_id)
                        print(f"Limit Node ID: {limit_id} from the {stage_name}") # *Print the limit node ID from the limit node of the stages
                        limit_node.add_controller(limit_controller) 
                        # limit_node.add_trigger(limit_trigger) # *Add the limit trigger to the limit node as an example
                        # limit_node_example = limit_node.get_node() # *Get the limit node as an example
                        # print(json.dumps(limit_node_example, indent=2)) # *Print the limit node with json format to see the changes.
                    case "flow":
                        # print(f"limit type in {stage_name} is {limit['type']}") # *Print the limit type from the stage in the JSON file according to the limits in the stages
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [0, trigger_limit_value]
                        limit_controller = FlowController(Flow_Algorithm_Type.PID_V1, 1, CurveInterpolationType.LINEAR, points_trigger, ReferenceType.TIME, 9)
                        limit_id = limit_node.get_node_id()
                        limit_trigger = FlowValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, trigger_limit_value, limit_id)
                        print(f"Limit Node ID: {limit_id} from the {stage_name}") # *Print the limit node ID from the limit node of the stages
                        limit_node.add_controller(limit_controller) 
                        # limit_node.add_trigger(limit_trigger) # *Add the limit trigger to the limit node as an example
                        # limit_node_example = limit_node.get_node() # *Get the limit node as an example
                        # print(json.dumps(limit_node_example, indent=2)) # *Print the limit node with json format to see the changes.
                    case "temperature":
                        # print(f"limit type in {stage_name} is {limit['type']}") # *Print the limit type from the stage in the JSON file according to the limits in the stages
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [0, trigger_limit_value]
                        limit_controller = TemperatureController(Temperature_Algorithm_Type.WATER, 4, CurveInterpolationType.LINEAR, points_trigger, ReferenceType.TIME, 9)
                        limit_id = limit_node.get_node_id()
                        limit_trigger = TemperatureValueTrigger(TemperatureSourceType.WATER, TriggerOperatorType.GREATER_THAN_OR_EQUAL, trigger_limit_value, limit_id)
                        print(f"Limit Node ID: {limit_id} from the {stage_name}") # *Print the limit node ID from the limit node of the stages
                        limit_node.add_controller(limit_controller) 
                        # limit_node.add_trigger(limit_trigger) # *Add the limit trigger to the limit node as an example
                        # limit_node_example = limit_node.get_node() # *Get the limit node as an example
                        # print(json.dumps(limit_node_example, indent=2)) # *Print the limit node with json format to see the changes.
                    case "power":
                        # print(f"limit type in {stage_name} is {limit['type']}") # *Print the limit type from the stage in the JSON file according to the limits in the stages
                        limit_node = Nodes(self.get_new_node_id())
                        trigger_limit_value = limit["value"]
                        points_trigger = [0, trigger_limit_value]
                        limit_controller = PowerController(Power_Algorithm_Type.SPRING, 1,  CurveInterpolationType.LINEAR, points_trigger, ReferenceType.TIME, 9)
                        limit_id = limit_node.get_node_id()
                        limit_trigger = PowerValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, trigger_limit_value, limit_id)
                        print(f"Limit Node ID: {limit_id} from the {stage_name}") # *Print the limit node ID from the limit node of the stages
                        limit_node.add_controller(limit_controller) 
                        # limit_node.add_trigger(limit_trigger) # *Add the limit trigger to the limit node as an example
                        # limit_node_example = limit_node.get_node() # *Get the limit node as an example
                        # print(json.dumps(limit_node_example, indent=2)) # *Print the limit node with json format to see the changes.
                    case _:
                        print(f"Limit type: {limit['type']} not found.")
                all_nodes.append(limit_node.get_node())
                limit_triggers.append(limit_trigger.get_trigger())
                # limit_node_example_complete = [limit_node.get_node(), limit_trigger.get_trigger()] # *Get the limit node and trigger as an example
                # print(json.dumps(limit_node_example_complete, indent=2)) # *Print the limit node and trigger with json format to see the changes.
            next_stage_node_id = self.get_new_node_id()
            current_node_id = next_stage_node_id
            print(f"Next Stage Node ID: {next_stage_node_id} after the {stage_name}") # *Print the next stage node ID from the next stage node of the stages
            print(F"Current Node ID: {current_node_id} from the stage {stage_name}") # *Print the current node ID from the current node of the stages
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
                        # print(f"Exit trigger find in {stage_name} with type: {exits['type']}") # *Print the exit trigger type from the exit triggers of the stages 
                        # print(json.dumps(exit_trigger.get_trigger(), indent=2)) # *Print the exit trigger with json format to see the changes.
                    case "weight":
                        exit_trigger_value = exits["value"]
                        if exits["relative"]:
                            reference_id = 3
                        else:
                            reference_id = 100
                        exit_trigger = WeightTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, reference_id, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        print(F"Next Stage Node ID after match: {next_stage_node_id} from the stage {stage_name}")
                        # print(f"Exit trigger find in {stage_name} with type: {exits['type']}") # *Print the exit trigger type from the exit triggers of the stages 
                        # print(json.dumps(exit_trigger.get_trigger(), indent=2)) # *Print the exit trigger with json format to see the changes.
                    case "pressure":
                        exit_trigger_value = exits["value"]
                        exit_trigger = PressureValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        print(F"Next Stage Node ID after match: {next_stage_node_id} from the stage {stage_name}")
                        # print(f"Exit trigger find in {stage_name} with type: {exits['type']}") # *Print the exit trigger type from the exit triggers of the stages 
                        # print(json.dumps(exit_trigger.get_trigger(), indent=2)) # *Print the exit trigger with json format to see the changes.
                    case "flow":
                        exit_trigger_value = exits["value"]
                        exit_trigger = FlowValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        print(F"Next Stage Node ID after match: {next_stage_node_id} from the stage {stage_name}")
                        # print(f"Exit trigger find in {stage_name} with type: {exits['type']}") # *Print the exit trigger type from the exit triggers of the stages 
                        # print(json.dumps(exit_trigger.get_trigger(), indent=2)) # *Print the exit trigger with json format to see the changes.
                    case "piston_position":
                        exit_trigger_value = exits["value"]
                        if exits["relative"]:
                            reference_id = 1
                        else:
                            reference_id = 0
                        exit_trigger = PistonPositionTrigger(TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, reference_id, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        print(F"Next Stage Node ID after match: {next_stage_node_id} from the stage {stage_name}")
                        # print(f"Exit trigger find in {stage_name} with type: {exits['type']}") # *Print the exit trigger type from the exit triggers of the stages 
                        # print(json.dumps(exit_trigger.get_trigger(), indent=2)) # *Print the exit trigger with json format to see the changes.
                    case "power":
                        exit_trigger_value = exits["value"]
                        exit_trigger = PowerValueTrigger(SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        print(F"Next Stage Node ID after match: {next_stage_node_id} from the stage {stage_name}")
                        # print(f"Exit trigger find in {stage_name} with type: {exits['type']}") # *Print the exit trigger type from the exit triggers of the stages 
                        # print(json.dumps(exit_trigger.get_trigger(), indent=2)) # *Print the exit trigger with json format to see the changes.
                    case "temperature":
                        exit_trigger_value = exits["value"]
                        exit_trigger = TemperatureValueTrigger(TemperatureSourceType.WATER, TriggerOperatorType.GREATER_THAN_OR_EQUAL, exit_trigger_value, next_stage_node_id)
                        exit_triggers.append(exit_trigger.get_trigger())
                        print(F"Next Stage Node ID after match: {next_stage_node_id} from the stage {stage_name}")
                        # print(f"Exit trigger find in {stage_name} with type: {exits['type']}") # *Print the exit trigger type from the exit triggers of the stages 
                        # print(json.dumps(exit_trigger.get_trigger(), indent=2)) # *Print the exit trigger with json format to see the changes.
                    case _:
                        print(f"Exit type: {exits['type']} not found.")
            # print(f"Exit triggers find in {stage_name}:") # *Print the exit triggers found in the stages
            # print(json.dumps(exit_triggers, indent=2)) # *Print the exit triggers with json format to see the changes.
            for limit_node in all_nodes[2:]:
                limit_node["triggers"] += [trigger for trigger in limit_triggers if trigger["next_node_id"] != limit_node["id"]]
                # print(f"Limit node triggers in {stage_name}:") # *Print the limit node triggers found in the stages
                # print(json.dumps(limit_node["triggers"], indent=2)) # *Print the limit node triggers with json format to see the changes.
                limit_node["triggers"] += exit_triggers
                limit_node_id = limit_node["id"] # *Get the limit node id from the limit node of the stages
                trigger_next_node_id = limit_node["triggers"][0]["next_node_id"] # *Get the next node id from the exit triggers of the stages
                print(f"Limit node triggers with exit triggers in {stage_name}") # *Print the limit node triggers with exit triggers found in the stages
                print(f"The limit node is type: {limit['type']} \nThe limit node id is : {limit_node_id}") # *Print the limit type from the stage in the JSON file according to the limits in the stages
                print(f"The trigger next node id is: {trigger_next_node_id} ") # *Print the next node id from the exit triggers of the stages
                # print(json.dumps(limit_node["triggers"], indent=2)) # *Print the limit node triggers with exit triggers with json format to see the changes.
            print(f"Limit nodes in {stage_name}:") # *Print the limit nodes found in the stages 
            
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
            print(f"Main Node ID: ({main_node_id}) after exit triggers ") # *Print the main node ID from the main node of the stages
            # print(f"Type from the {stage_name}: {type_main_controller}") # *Print the type from the stage in the JSON file
            # print (f"Dynamics from the {stage_name}:")
            # print(f"Points: {points_main_controller}") # *Print the points from the stage in the JSON file
            # print(f"Over: {over_main_controller}") # *Print the over from the stage in the JSON file
            # print(f"Interpolation: {interpolation_main_controller}") # *Print the interpolation from the stage in the JSON file
            
            match type_main_controller:
                case "pressure":
                    main_controller = PressureController(Pressure_Algorithm_Type.PID_V1, 7, interpolation_dict[interpolation_main_controller], points_main_controller, over_dict[over_main_controller], 9)
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = PressureCurveTrigger(SourceType.RAW,TriggerOperatorType.GREATER_THAN_OR_EQUAL, main_curve_id, main_node_id)
                    main_trigger = main_trigger.get_trigger()
                    print(f"Main node id: {main_node_id} after main controller from the {stage_name} and type {type_main_controller}") # *Print the main node ID from the main node of the stages
                    # print(f"The type is {type_main_controller} from the {stage_name}") # *Print the type from the stage in the JSON file
                    # print(f"Main Node ID: {main_node.get_node_id()} from the {stage_name}") # *Print the main node ID from the main node of the stages
                    # print(json.dumps(main_node.get_node(), indent=2)) # *Print the main node with json format to see the changes.
                case "flow":
                    main_controller = FlowController(Flow_Algorithm_Type.PID_V1, 8, interpolation_dict[interpolation_main_controller], points_main_controller, over_dict[over_main_controller], 9)
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = FlowCurveTrigger(SourceType.RAW,TriggerOperatorType.GREATER_THAN_OR_EQUAL, main_curve_id, main_node_id)
                    main_trigger = main_trigger.get_trigger()
                    print(f"Main node id: {main_node_id} after main controller from the {stage_name} and type {type_main_controller}") # *Print the main node ID from the main node of the stages
                    # print(f"The type is {type_main_controller} from the {stage_name}") # *Print the type from the stage in the JSON file
                    # print(f"Main Node ID: {main_node.get_node_id()} from the {stage_name}") # *Print the main node ID from the main node of the stages
                    # print(json.dumps(main_node.get_node(), indent=2)) # *Print the main node with json format to see the changes.
                case "temperature":
                    main_controller = TemperatureController(Temperature_Algorithm_Type.WATER, 4, interpolation_dict[interpolation_main_controller], points_main_controller, over_dict[over_main_controller], 9)
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = TemperatureCurveTrigger(SourceType.RAW,TriggerOperatorType.GREATER_THAN_OR_EQUAL, main_curve_id, 4)
                    main_trigger = main_trigger.get_trigger()
                    print(f"Main node id: {main_node_id} after main controller from the {stage_name} and type {type_main_controller}") # *Print the main node ID from the main node of the stages
                    # print(f"The type is {type_main_controller} from the {stage_name}") # *Print the type from the stage in the JSON file
                    # print(f"Main Node ID: {main_node.get_node_id()} from the {stage_name}") # *Print the main node ID from the main node of the stages
                    # print(json.dumps(main_node.get_node(), indent=2)) # *Print the main node with json format to see the changes.
                case "power":
                    main_controller = PowerController(Power_Algorithm_Type.SPRING, 1, interpolation_dict[interpolation_main_controller], points_main_controller, over_dict[over_main_controller], 9)
                    main_curve_id = main_controller.get_curve_id()
                    main_node.add_controller(main_controller)
                    main_trigger = PowerCurveTrigger(SourceType.RAW,TriggerOperatorType.GREATER_THAN_OR_EQUAL, main_curve_id, main_node_id)
                    main_trigger = main_trigger.get_trigger()
                    print(f"Main node id: {main_node_id} after main controller from the {stage_name} and type {type_main_controller}") # *Print the main node ID from the main node of the stages
                    # print(f"The type is {type_main_controller} from the {stage_name}") # *Print the type from the stage in the JSON file
                    # print(f"Main Node ID: {main_node.get_node_id()} from the {stage_name}") # *Print the main node ID from the main node of the stages
                    # print(json.dumps(main_node.get_node(), indent=2)) # *Print the main node with json format to see the changes.
                case _:
                    print(f"Type: {type_main_controller} not found.")
                    
            
            
            for limit_node in all_nodes[2:]:
                limit_node["triggers"].append(main_trigger)            
            # print(f"limit node from the {stage_name}:") # *Print the limit node from the stages
            # print(json.dumps(limit_nodes, indent=2)) # *Print the limit nodes with the main controller. 
            print(f"Main Node ID: {main_node.get_node_id()} before build the complex stages") # *Print the main node ID from the main node of the stages
            # print(json.dumps(main_node.get_node(), indent=2)) # *Print the main node with json format to see the changes.
            complex_stages.append({
                "name": f"{stage_name}",
                "nodes": all_nodes
            })
            
        #     # Id's debugging
        #     print(f"Init Node ID: {init_node.get_node_id()} from the stage {stage_name} after the complex stage")# *Print the ID of the init node.
        #     print(f"Main Node ID: {main_node.get_node_id()} from the stage {stage_name} after the complex stage") # *Print the ID of the main node.
        #     print(f"Limit Nodes ID {limit_id} from the stage {stage_name} after the complex stage") # *Print the ID of the limit nodes.
        #     print(f"Next Stage Node ID: {next_stage_node_id} from the stage {stage_name} after the complex stage") # *Print the next stage node ID from the next stage node of the stages
            
        # # print(f"Complex stage nodes from the {stage_name}:") # *Print the complex stages with the stage's name
        # # print(json.dumps(complex_stages, indent=2)) # *Print the complex stages with json format to see the changes.

        return all_nodes[2:]
    
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
    print(json.dumps(complex_node, indent=2))
    
    
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
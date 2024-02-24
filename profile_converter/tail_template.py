import json
from stages import *

class TailTemplate:
    
    def __init__(self):
        self.data = {}
        
    def retracting_stage(self, click_to_purge: bool):
        
        self.retracting_stage_build = Stages("retracting")
        self.init_node_retracting = Nodes(25)
        self.position_reference_retracting = PositionReferenceController(3)
        self.weight_reference_retracting = WeightReferenceController(4)
        self.exit_retracting = ExitTrigger(24)
        self.init_node_retracting.add_controller(self.position_reference_retracting)
        self.init_node_retracting.add_controller(self.weight_reference_retracting)
        self.init_node_retracting.add_trigger(self.exit_retracting)
        self.node_24_retracting = Nodes(24)
        self.move_piston_retracting = SpeedController(speed = 4, direction = DirectionType.BACKWARD)
        self.piston_position_retracting = PistonPositionTrigger(TriggerOperatorType.LESS_THAN_OR_EQUAL, -4, 3, 27)
        if click_to_purge:
            self.next_node_retracting = 30
        else:
            self.next_node_retracting = 48
        self.button_trigger_retracting = ButtonTrigger(ButtonSourceType.ENCODER_BUTTON, next_node_id = self.next_node_retracting)
        self.node_24_retracting.add_controller(self.move_piston_retracting)
        self.node_24_retracting.add_trigger(self.piston_position_retracting)
        self.node_24_retracting.add_trigger(self.button_trigger_retracting)
        self.node_27_retracting = Nodes(27)
        self.move_piston_retracting_2 = SpeedController(speed = 6, direction = DirectionType.BACKWARD)
        self.speed_trigger_retracting = SpeedTrigger(TriggerOperatorType.EQUAL, 0, self.next_node_retracting)
        self.button_trigger_retracting_1 = ButtonTrigger(ButtonSourceType.ENCODER_BUTTON, next_node_id = self.next_node_retracting)
        self.node_27_retracting.add_controller(self.move_piston_retracting_2)
        self.node_27_retracting.add_trigger(self.speed_trigger_retracting)
        self.node_27_retracting.add_trigger(self.button_trigger_retracting_1)
        self.retracting_stage_build.add_node(self.init_node_retracting)
        self.retracting_stage_build.add_node(self.node_24_retracting)
        self.retracting_stage_build.add_node(self.node_27_retracting)
        
        return self.retracting_stage_build.get_stage()
    
    def click_to_purge_stage(self):
        
        self.click_to_purge_build = Stages("click to purge")
        self.init_node_click_to_purge = Nodes(30)
        self.log_click_to_purge = LogController(MessageType.PURGE_CLICK)
        self.button_trigger_click_to_purge = ButtonTrigger(ButtonSourceType.ENCODER_BUTTON, next_node_id = 31)
        self.init_node_click_to_purge.add_controller(self.log_click_to_purge)
        self.init_node_click_to_purge.add_trigger(self.button_trigger_click_to_purge)
        self.click_to_purge_build.add_node(self.init_node_click_to_purge)
        
        return self.click_to_purge_build.get_stage()
    
    def remove_cup_stage(self):
        
        self.remove_cup_build = Stages("remove cup")
        self.init_node_remove_cup = Nodes(48)
        self.time_reference_remove_cup = TimeReferenceController(15)
        self.weight_trigger_remove_cup = WeightTrigger(operator = TriggerOperatorType.LESS_THAN_OR_EQUAL, value = -5, next_node_id = 31, weight_reference = 4)
        self.button_trigger_click_to_purge = ButtonTrigger(ButtonSourceType.ENCODER_BUTTON, next_node_id = 31)
        self.init_node_remove_cup.add_controller(self.time_reference_remove_cup)
        self.init_node_remove_cup.add_trigger(self.weight_trigger_remove_cup)
        self.init_node_remove_cup.add_trigger(self.button_trigger_click_to_purge)
        self.node_48_remove_cup = Nodes(48)
        self.timer_trigger_remove_cup = TimerTrigger(TriggerOperatorType.GREATER_THAN_OR_EQUAL, 5, 15, 31)
        self.button_trigger_click_to_purge_1 = ButtonTrigger(ButtonSourceType.ENCODER_BUTTON, next_node_id = 31)
        self.node_48_remove_cup.add_trigger(self.timer_trigger_remove_cup)
        self.node_48_remove_cup.add_trigger(self.button_trigger_click_to_purge_1)
        self.remove_cup_build.add_node(self.init_node_remove_cup)
        self.remove_cup_build.add_node(self.node_48_remove_cup)
        
        return self.remove_cup_build.get_stage()
    
    def purge_stage(self):
        
        self.purge_build = Stages("purge")
        self.init_node_purge = Nodes(31)
        self.move_piston_purge = SpeedController(speed = 6)
        self.time_reference_purge = TimeReferenceController(8)
        self.pressure_trigger_purge = PressureValueTrigger(operator = TriggerOperatorType.GREATER_THAN_OR_EQUAL, value = 6, next_node_id = 32)
        self.piston_position_purge = PistonPositionTrigger(value = 78, next_node_id = -2, position_reference = 0)
        self.button_trigger_purge = ButtonTrigger(ButtonSourceType.ENCODER_BUTTON, next_node_id = -2)
        self.init_node_purge.add_controller(self.move_piston_purge)
        self.init_node_purge.add_controller(self.time_reference_purge)
        self.init_node_purge.add_trigger(self.pressure_trigger_purge)
        self.init_node_purge.add_trigger(self.piston_position_purge)
        self.init_node_purge.add_trigger(self.button_trigger_purge)
        self.node_32_purge = Nodes(32)
        self.pressure_controller_purge = PressureController(curve_id = 5, reference_id = 8)
        self.piston_position_purge_1 = PistonPositionTrigger(value = 78, next_node_id = -2, position_reference = 0)
        self.button_trigger_purge_1 = ButtonTrigger(ButtonSourceType.ENCODER_BUTTON, next_node_id = -2)
        self.node_32_purge.add_controller(self.pressure_controller_purge)
        self.node_32_purge.add_trigger(self.piston_position_purge_1)
        self.node_32_purge.add_trigger(self.button_trigger_purge_1)
        self.purge_build.add_node(self.init_node_purge)
        self.purge_build.add_node(self.node_32_purge)
        
        return self.purge_build.get_stage()
    
    def end_stage(self):
        
        self.end_build = Stages("end stage")
        self.init_node_end = Nodes(-2)
        self.end_profile = EndProfile()
        self.init_node_end.add_controller(self.end_profile)
        self.end_build.add_node(self.init_node_end)
        
        return self.end_build.get_stage()
    
if __name__ == '__main__':
        
        tail_template = TailTemplate()
        retracting_stage = tail_template.retracting_stage(True)
        print(json.dumps(retracting_stage, indent = 2))
        
        click_to_purge_stage = tail_template.click_to_purge_stage()
        print(json.dumps(click_to_purge_stage, indent = 2))
        
        remove_cup_stage = tail_template.remove_cup_stage()
        print(json.dumps(remove_cup_stage, indent = 2))
        
        purge_stage = tail_template.purge_stage()
        print(json.dumps(purge_stage, indent = 2))
        
        end_stage = tail_template.end_stage()
        print(json.dumps(end_stage, indent = 2))

        
        
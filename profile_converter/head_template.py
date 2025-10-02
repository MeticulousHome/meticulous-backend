import json

from profile_converter.controllers import (
    LogController,
    PositionReferenceController,
    PressureController,
    SpeedController,
    TareController,
    TemperatureController,
    TimeReferenceController,
    WeightReferenceController,
)
from profile_converter.enums import (
    ButtonSourceType,
    CurveInterpolationType,
    DirectionType,
    MessageType,
    PressureAlgorithmType,
    ReferenceType,
    SourceType,
    SpeedAlgorithmType,
    TemperatureAlgorithmType,
    TemperatureSourceType,
    TriggerOperatorType,
)
from profile_converter.nodes import Nodes
from profile_converter.stages import Stages
from profile_converter.triggers import (
    ButtonTrigger,
    ExitTrigger,
    PistonPositionTrigger,
    PressureValueTrigger,
    SpeedTrigger,
    TemperatureValueTrigger,
    TimerTrigger,
    WaterDetectionTrigger,
)


class HeadProfile:
    def __init__(self):
        self.data = {}
        # try:
        #     self.click_to_start = click_to_start
        # except:
        #     self.click_to_start = True
        #     print('Warning: click_to_start is not defined, defaulting to True')

    def purge_stage(self):
        self.purge_stage_build = Stages("purge")
        self.initial_node_purge = Nodes(-1)
        self.piston_position_purge = PistonPositionTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL, 78, 0, 45
        )
        self.piston_position_1_purge = PistonPositionTrigger(
            TriggerOperatorType.LESS_THAN, 78, 0, 6
        )
        self.button_trigger_purge = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=45
        )
        self.initial_node_purge.add_trigger(self.piston_position_purge)
        self.initial_node_purge.add_trigger(self.piston_position_1_purge)
        self.initial_node_purge.add_trigger(self.button_trigger_purge)
        self.node_6_purge = Nodes(6)
        self.time_reference_purge = TimeReferenceController(3)
        self.move_piston_purge = SpeedController(
            SpeedAlgorithmType.EASE_IN, 6, DirectionType.FORWARD
        )
        self.pressure_trigger_purge = PressureValueTrigger(
            SourceType.RAW, TriggerOperatorType.GREATER_THAN_OR_EQUAL, 6, 11
        )
        self.piston_position_2_purge = PistonPositionTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL, 78, 0, 45
        )
        self.button_trigger_purge_1 = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=45
        )
        self.node_6_purge.add_controller(self.move_piston_purge)
        self.node_6_purge.add_controller(self.time_reference_purge)
        self.node_6_purge.add_trigger(self.pressure_trigger_purge)
        self.node_6_purge.add_trigger(self.piston_position_2_purge)
        self.node_6_purge.add_trigger(self.button_trigger_purge_1)
        self.node_11_purge = Nodes(11)
        self.pressure_controller_purge = PressureController(
            PressureAlgorithmType.PID_V1,
            1,
            CurveInterpolationType.LINEAR,
            reference_kind=ReferenceType.TIME,
            reference_id=3,
        )
        self.piston_position_3_purge = PistonPositionTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL, 78, 0, 45
        )
        self.button_trigger_purge_2 = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=45
        )
        self.node_11_purge.add_controller(self.pressure_controller_purge)
        self.node_11_purge.add_trigger(self.piston_position_3_purge)
        self.node_11_purge.add_trigger(self.button_trigger_purge_2)
        self.purge_stage_build.add_node(self.initial_node_purge)
        self.purge_stage_build.add_node(self.node_6_purge)
        self.purge_stage_build.add_node(self.node_11_purge)

        return self.purge_stage_build.get_stage()

    def water_detection_stage(self, water_detection: bool):
        self.water_detection_stage_build = Stages("water detection")
        self.initial_node_water_detection = Nodes(45)
        self.time_reference_water_detection = TimeReferenceController(12)
        self.exit_water_detection = ExitTrigger(9)
        self.initial_node_water_detection.add_controller(self.time_reference_water_detection)
        self.initial_node_water_detection.add_trigger(self.exit_water_detection)
        self.node_9_water_detection = Nodes(9)
        self.time_reference_water_detection_1 = TimeReferenceController(2)
        if water_detection:
            self.next_node_water = 15
        else:
            self.next_node_water = 12
        self.water_detection_trigger = WaterDetectionTrigger(
            water_detection, self.next_node_water
        )
        self.button_trigger_water_detection = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=15
        )
        self.node_9_water_detection.add_controller(self.time_reference_water_detection_1)
        self.node_9_water_detection.add_trigger(self.water_detection_trigger)
        self.node_9_water_detection.add_trigger(self.button_trigger_water_detection)
        self.node_12_water_detection = Nodes(12)
        self.log_water_detection = LogController()
        self.time_trigger_water_detection = TimerTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL, 2, 2, 9
        )
        self.time_trigger_water_detection_1 = TimerTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL, 100, 12, -2
        )
        self.button_trigger_water_detection_1 = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=15
        )
        self.node_12_water_detection.add_controller(self.log_water_detection)
        self.node_12_water_detection.add_trigger(self.time_trigger_water_detection)
        self.node_12_water_detection.add_trigger(self.time_trigger_water_detection_1)
        self.node_12_water_detection.add_trigger(self.button_trigger_water_detection_1)
        self.water_detection_stage_build.add_node(self.initial_node_water_detection)
        self.water_detection_stage_build.add_node(self.node_9_water_detection)
        self.water_detection_stage_build.add_node(self.node_12_water_detection)

        return self.water_detection_stage_build.get_stage()

    def heating_stage(self, target_temperature: float, click_to_start: bool):
        self.heating_stage_build = Stages("heating")
        self.heating_node = Nodes(15)
        self.points_heating = [0, target_temperature]
        if click_to_start:
            self.next_node_heating = 16
        else:
            self.next_node_heating = 17
        self.heating_controller = TemperatureController(
            curve_id=2, points=self.points_heating, reference_id=2
        )
        self.position_reference_heating = PositionReferenceController(1)
        self.temperature_trigger_heating = TemperatureValueTrigger(
            TemperatureSourceType.WATER,
            TriggerOperatorType.GREATER_THAN_OR_EQUAL,
            target_temperature,
            self.next_node_heating,
        )
        self.timer_trigger_heating = TimerTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL, 900, 2, -2
        )
        self.button_trigger_heating = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=self.next_node_heating
        )
        self.heating_node.add_controller(self.heating_controller)
        self.heating_node.add_controller(self.position_reference_heating)
        self.heating_node.add_trigger(self.temperature_trigger_heating)
        self.heating_node.add_trigger(self.timer_trigger_heating)
        self.heating_node.add_trigger(self.button_trigger_heating)
        self.heating_stage_build.add_node(self.heating_node)

        return self.heating_stage_build.get_stage()

    def click_to_start_stage(self):
        self.click_start_stage_build = Stages("click to start")
        self.initial_node_click_to_start = Nodes(16)
        self.log_click_to_start = LogController(MessageType.START_CLICK)
        self.button_trigger_click_to_start = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=17
        )
        self.initial_node_click_to_start.add_controller(self.log_click_to_start)
        self.initial_node_click_to_start.add_trigger(self.button_trigger_click_to_start)
        self.click_start_stage_build.add_node(self.initial_node_click_to_start)

        return self.click_start_stage_build.get_stage()

    def retracting_stage(self):
        self.retracting_stage_build = Stages("retracting")
        self.initial_node_retracting = Nodes(17)
        self.move_piston_retracting = SpeedController(speed=4, direction=DirectionType.BACKWARD)
        self.piston_position_retracting = PistonPositionTrigger(
            TriggerOperatorType.LESS_THAN_OR_EQUAL, -2, 1, 18
        )
        self.button_trigger_retracting = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=23
        )
        self.initial_node_retracting.add_controller(self.move_piston_retracting)
        self.initial_node_retracting.add_trigger(self.piston_position_retracting)
        self.initial_node_retracting.add_trigger(self.button_trigger_retracting)
        self.node_18_retracting = Nodes(18)
        self.move_piston_retracting_1 = SpeedController(
            speed=6, direction=DirectionType.BACKWARD
        )
        self.piston_speed_trigger_retracting = SpeedTrigger(TriggerOperatorType.EQUAL, 0, 21)
        self.button_trigger_retracting_1 = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=23
        )
        self.node_18_retracting.add_controller(self.move_piston_retracting_1)
        self.node_18_retracting.add_trigger(self.piston_speed_trigger_retracting)
        self.node_18_retracting.add_trigger(self.button_trigger_retracting_1)
        self.node_21_retracting = Nodes(21)
        self.tare_retracting = TareController()
        self.time_reference_retracting = TimeReferenceController(4)
        self.exit_retracting = ExitTrigger(22)
        self.node_21_retracting.add_controller(self.tare_retracting)
        self.node_21_retracting.add_controller(self.time_reference_retracting)
        self.node_21_retracting.add_trigger(self.exit_retracting)
        self.node_22_retracting = Nodes(22)
        self.weight_reference_retracting = WeightReferenceController(1)
        self.time_trigger_retracting = TimerTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL, 2, 4, 23
        )
        self.button_trigger_retracting_2 = ButtonTrigger(
            ButtonSourceType.ENCODER_BUTTON, next_node_id=23
        )
        self.node_22_retracting.add_controller(self.weight_reference_retracting)
        self.node_22_retracting.add_trigger(self.time_trigger_retracting)
        self.node_22_retracting.add_trigger(self.button_trigger_retracting_2)
        self.retracting_stage_build.add_node(self.initial_node_retracting)
        self.retracting_stage_build.add_node(self.node_18_retracting)
        self.retracting_stage_build.add_node(self.node_21_retracting)
        self.retracting_stage_build.add_node(self.node_22_retracting)

        return self.retracting_stage_build.get_stage()

    def closing_valve_stage(self, end_node: int):
        self.closing_valve_stage_build = Stages("closing valve")
        self.closing_valve_node = Nodes(23)
        self.temperature_closing_valve = TemperatureController(
            TemperatureAlgorithmType.CYLINDER, 6, points=[0, 25], reference_id=9
        )
        self.move_piston_closing_valve = SpeedController(speed=5)
        self.time_reference_closing_valve = TimeReferenceController(1)
        self.position_trigger_closing_valve = PistonPositionTrigger(
            TriggerOperatorType.GREATER_THAN_OR_EQUAL, 75, 0, end_node
        )
        self.closing_valve_node.add_controller(self.temperature_closing_valve)
        self.closing_valve_node.add_controller(self.move_piston_closing_valve)
        self.closing_valve_node.add_controller(self.time_reference_closing_valve)
        self.closing_valve_node.add_trigger(self.position_trigger_closing_valve)
        self.closing_valve_stage_build.add_node(self.closing_valve_node)

        return self.closing_valve_stage_build.get_stage()


if __name__ == "__main__":
    head_profile = HeadProfile()
    purge_stage_example = head_profile.purge_stage()
    print(json.dumps(purge_stage_example, indent=2))

    water_detection_stage_example = head_profile.water_detection_stage(False)
    print(json.dumps(water_detection_stage_example, indent=2))

    heating_stage_example = head_profile.heating_stage(80, False)
    print(json.dumps(heating_stage_example, indent=2))

    click_to_start_stage_example = head_profile.click_to_start_stage()
    print(json.dumps(click_to_start_stage_example, indent=2))

    retracting_stage_example = head_profile.retracting_stage()
    print(json.dumps(retracting_stage_example, indent=2))

    closing_valve_stage_example = head_profile.closing_valve_stage(11000)
    print(json.dumps(closing_valve_stage_example, indent=2))

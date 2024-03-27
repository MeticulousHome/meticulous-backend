from dataclasses import dataclass, replace
from enum import Enum, auto, unique
import re
import math

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

colorSensorRegex = None


def safeFloat(val):
    convert = float(val)
    if math.isnan(convert):
        return 0
    return convert

@dataclass
class SensorData:
    """Class respresenting the current state of all sensors"""

    external_1: float = 0.0
    external_2: float = 0.0
    bar_up: float = 0.0
    bar_mid_up: float = 0.0
    bar_mid_down: float = 0.0
    bar_down: float = 0.0
    tube: float = 0.0
    valve: float = 0.0
    motor_position: float = 0.0
    motor_speed: float = 0.0
    motor_power: float = 0.0
    motor_current: float = 0.0
    bandheater_power: float = 0.0
    preassure_sensor: float = 0.0
    adc_0: float = 0.0
    adc_1: float = 0.0
    adc_2: float = 0.0
    adc_3: float = 0.0

    def from_color_coded_args(colorSeperatedArgs):
        global colorSensorRegex
        if colorSensorRegex is None:
            startColor = "\033\\[1;(31|32|33|34|35|36)m"
            endColor = "\033\\[0m"
            colorSensorRegex = re.compile(f"{startColor} [a-z0-9_]*{endColor}")
        colorSeperatedArgs = colorSensorRegex.sub(',', colorSeperatedArgs)
        args = colorSeperatedArgs.split(",")
        if args[0] == "":
            args = args[1:]
        return SensorData.from_args(args)

    def from_args(args):
        try:
            data = SensorData(
                external_1=safeFloat(args[0]),
                external_2=safeFloat(args[1]),
                bar_up=safeFloat(args[2]),
                bar_mid_up=safeFloat(args[3]),
                bar_mid_down=safeFloat(args[4]),
                bar_down=safeFloat(args[5]),
                tube=safeFloat(args[6]),
                valve=safeFloat(args[7]),
                motor_position=safeFloat(args[8]),
                motor_speed=safeFloat(args[9]),
                motor_power=safeFloat(args[10]),
                motor_current=safeFloat(args[11]),
                bandheater_power=safeFloat(args[12]),
                preassure_sensor=safeFloat(args[13]),
                adc_0=safeFloat(args[14]),
                adc_1=safeFloat(args[15]),
                adc_2=safeFloat(args[16]),
                adc_3=safeFloat(args[17]),
            )
        except Exception as e:
            logger.warning(f"Failed to parse SensorData: {args}", exc_info=e)
            return None
        return data

    def to_sio_temperatures(self):
        return {
            "t_ext_1": self.external_1,
            "t_ext_2": self.external_2,
            "t_bar_up": self.bar_up,
            "t_bar_mu": self.bar_mid_up,
            "t_bar_md": self.bar_mid_down,
            "t_bar_down": self.bar_down,
            "t_tube": self.tube,
            "t_valv": self.valve
        }

    def to_sio_communication(self):
        return {
            "p": self.preassure_sensor,
            "a_0": self.adc_0,
            "a_1": self.adc_1,
            "a_2": self.adc_2,
            "a_3": self.adc_3
        }

    def to_sio_actuators(self):
        return {
            "m_pos": self.motor_position,
            "m_spd": self.motor_speed,
            "m_pwr": self.motor_power,
            "m_cur": self.motor_current,
            "bh_pwr": self.bandheater_power
        }


@dataclass
class ESPInfo:
    """Class respresenting the current ESPs firmware and status"""
    firmwareV: str = "0.0.0"
    fanStatus: bool = False
    mainVoltage: float = 0.0

    def from_args(args):
        try:
            info = ESPInfo(args[0], args[1] == "on", float(args[2]))
        except Exception as e:
            logger.warning(f"Failed to parse ESPInfo: {args}", exc_info=e)
            return None
        return info

    def to_sio(self):
        return {
            "firmwareV": self.firmwareV,
            "fanStatus": self.fanStatus,
            "mainVoltage": self.mainVoltage,
        }

class MachineStatus():
    # Enum representing the events from the machine
    IDLE = "idle"
    HEATING = "heating"
    INFUSION = "infusion"
    PREINFUSION = "preinfusion"
    PURGE = "purge"
    RETRACTING = "retracting"
    CLOSING_VALVE = "closing valve"
    SPRING = "spring"


@dataclass
class ShotData:
    """Class respresenting a Datapoint of the machine in time, used to track a shot"""

    pressure: float = 0.0
    flow: float = 0.0
    weight: float = 0.0
    temperature: float = 20.0
    status: str = ""
    profile: str = ""
    time: int = -1

    def clone_with_time(self, shot_start_time):
        return replace(self, time=shot_start_time)
    
    def from_args(args):
        try:
            s = args[4].strip("\r\n")
            status = s
        except:
            status = None

        try:
            profile = args[5].strip("\r\n")
        except:
            profile = None

        try:
            data = ShotData(float(args[0]),
                            float(args[1]),
                            float(args[2]),
                            float(args[3]),
                            status,
                            profile)
        except Exception as e:
            logger.warning(f"Failed to parse ShotData: {args}", exc_info=e)
            return None
        return data

    def to_sio(self):
        data = {
            "name": self.status,
            "sensors": {
                "p": self.pressure,
                "f": self.flow,
                "w": self.weight,
                "t": self.temperature,
            },
            "time": self.time,
            "profile": self.profile,
        }
        return data


@unique
class ButtonEventEnum(Enum):
    # Enum representing the events from the machine
    ENCODER_CLOCKWISE = auto()
    ENCODER_COUNTERCLOCKWISE = auto()
    ENCODER_PUSH = auto()
    ENCODER_DOUBLE = auto()
    ENCODER_LONG = auto()
    TARE = auto()
    TARE_DOUBLE = auto()
    TARE_LONG = auto()
    TARE_SUPER_LONG = auto()
    CONTEXT = auto()
    ENCODER_PRESSED = auto()
    ENCODER_RELEASED = auto()
    # Failure type
    UNKNOWN = auto()

    @classmethod
    def _missing_(cls, value):
        return cls.UNKNOWN

    @classmethod
    def from_str(cls, type_str):
        event_lookup = {
            "CW": "ENCODER_CLOCKWISE",
            "CCW": "ENCODER_COUNTERCLOCKWISE",
            "push": "ENCODER_PUSH",
            "pu_d": "ENCODER_DOUBLE",
            "elng": "ENCODER_LONG",
            "tare": "TARE",
            "ta_d": "TARE_DOUBLE",
            "ta_l": "TARE_LONG",
            "ta_sl": "TARE_SUPER_LONG",
            "strt": "CONTEXT",
            "cntx": "CONTEXT",
            "encoder_button_pressed" : "ENCODER_PRESSED",
            "encoder_button_released" : "ENCODER_RELEASED",
        }

        if event_lookup.get(type_str) is not None:
            type_str = event_lookup.get(type_str)

        return cls[type_str.upper()]


@dataclass
class ButtonEventData:
    """Class respresenting an pysical button Event"""

    event: "ButtonEventEnum"
    time_since_last_event: int = 0

    def from_args(args):
        try:
            time_since_last_event = 0

            try:
                if len(args) > 1:
                    if args[1] == "9999+++":
                        time_since_last_event = 10000
                    else:
                        time_since_last_event = int(args[1])
            except ValueError:
                pass

            event = ButtonEventData(ButtonEventEnum.from_str(
                args[0]), time_since_last_event)
        except Exception as e:
            logger.warning(f"Failed to parse EncoderEventData: {args}", exc_info=e)
            return None

        return event

    def to_sio(self):
        return {
            "type": self.event.name,
            "time_since_last_event": int(self.time_since_last_event),
        }


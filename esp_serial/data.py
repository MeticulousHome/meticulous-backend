from dataclasses import dataclass, replace
from enum import Enum, auto, unique
import re
import math

from log import MeticulousLogger
from config import MeticulousConfig, GET_ACCESSORY_DATA, CONFIG_USER

logger = MeticulousLogger.getLogger(__name__)

colorSensorRegex = None


def safeFloat(val):
    convert = float(val)
    if math.isnan(convert):
        return 0
    return convert


def safe_float_with_nan(value):
    try:
        f_value = float(value)
        if math.isnan(f_value):
            return "NaN"
        return f_value
    except ValueError:
        return "NaN"


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
    lam_temp: float = 0.0
    motor_position: float = 0.0
    motor_speed: float = 0.0
    motor_power: float = 0.0
    motor_current: float = 0.0
    bandheater_power: float = 0.0
    bandheater_current: float = 0.0
    pressure_sensor: float = 0.0
    adc_0: float = 0.0
    adc_1: float = 0.0
    adc_2: float = 0.0
    adc_3: float = 0.0
    water_status: bool = False
    motor_thermistor: float = 0.0

    def from_color_coded_args(colorSeperatedArgs):
        global colorSensorRegex
        if colorSensorRegex is None:
            startColor = "\033\\[1;(31|32|33|34|35|36)m"
            endColor = "\033\\[0m"
            colorSensorRegex = re.compile(f"{startColor} [a-z0-9_]*{endColor}")
        colorSeperatedArgs = colorSensorRegex.sub(",", colorSeperatedArgs)
        args = colorSeperatedArgs.split(",")
        if args[0] == "":
            args = args[1:]
        return SensorData.from_args(args)

    def from_args(args):
        try:

            if len(args) >= 21:
                water_status = args[20].lower() == "true"
            else:
                water_status = False

            if len(args) >= 20:
                data = SensorData(
                    external_1=safeFloat(args[0]),
                    external_2=safeFloat(args[1]),
                    bar_up=safeFloat(args[2]),
                    bar_mid_up=safeFloat(args[3]),
                    bar_mid_down=safeFloat(args[4]),
                    bar_down=safeFloat(args[5]),
                    tube=safeFloat(args[6]),
                    valve=safeFloat(args[7]),
                    lam_temp=safeFloat(args[8]),
                    motor_position=safeFloat(args[9]),
                    motor_speed=safeFloat(args[10]),
                    motor_power=safeFloat(args[11]),
                    motor_current=safeFloat(args[12]),
                    bandheater_current=safeFloat(args[13]),
                    bandheater_power=safeFloat(args[14]),
                    pressure_sensor=safeFloat(args[15]),
                    adc_0=safeFloat(args[16]),
                    adc_1=safeFloat(args[17]),
                    adc_2=safeFloat(args[18]),
                    adc_3=safeFloat(args[19]),
                    water_status=water_status,
                )
                if MeticulousConfig[CONFIG_USER][GET_ACCESSORY_DATA]:
                    data.motor_thermistor = safeFloat(args[21])
                else:
                    data.motor_thermistor = 0.0
            else:
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
                    pressure_sensor=safeFloat(args[13]),
                    adc_0=safeFloat(args[14]),
                    adc_1=safeFloat(args[15]),
                    adc_2=safeFloat(args[16]),
                    adc_3=safeFloat(args[17]),
                    water_status=water_status,
                )
                if MeticulousConfig[CONFIG_USER][GET_ACCESSORY_DATA]:
                    data.motor_thermistor = safeFloat(args[21])
                else:
                    data.motor_thermistor = 0.0

        except Exception as e:
            logger.warning(
                f"Failed to parse SensorData ({len(args)}): {args}", exc_info=e
            )
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
            "t_valv": self.valve,
            "lam_temp": self.lam_temp,
        }

    def to_sio_communication(self):
        return {
            "p": self.pressure_sensor,
            "a_0": self.adc_0,
            "a_1": self.adc_1,
            "a_2": self.adc_2,
            "a_3": self.adc_3,
        }

    def to_sio_actuators(self):
        return {
            "m_pos": self.motor_position,
            "m_spd": self.motor_speed,
            "m_pwr": self.motor_power,
            "m_cur": self.motor_current,
            "bh_pwr": self.bandheater_power,
            "bh_cur": self.bandheater_current,
        }

    def to_sio_water_status(self):
        return {"water_status": self.water_status}

    def to_sio_accessory_data(self):
        return {"motor_thermistor": self.motor_thermistor}


@dataclass
class ESPInfo:
    """Class respresenting the current ESPs firmware and status"""

    firmwareV: str = "0.0.0"
    espPinout: int = 0
    mainVoltage: float = 0.0

    color: str = ""
    serialNumber: str = ""
    batchNumber: str = ""
    buildDate: str = ""

    def from_args(args):
        espPinout = 0
        try:
            # This used to be the fan status. To not break on old firmware we check parseability
            espPinout = int(args[1])
        except Exception:
            pass
        try:
            if len(args) >= 7:
                info = ESPInfo(
                    args[0],
                    espPinout,
                    float(args[2]),
                    args[3],
                    args[4],
                    args[5],
                    args[6],
                )
            else:
                info = ESPInfo(args[0], espPinout, float(args[2]))
        except Exception as e:
            logger.warning(f"Failed to parse ESPInfo: {args}", exc_info=e)
            return None
        return info

    def to_sio(self):
        return {
            "firmwareV": self.firmwareV,
            "espPinout": self.espPinout,
            "mainVoltage": self.mainVoltage,
            "color": self.color,
            "serialNumber": self.serialNumber,
            "batchNumber": self.batchNumber,
            "buildDate": self.buildDate,
        }


# From ESP32 to backend
class MachineStatus:
    # Enum representing the events from the machine
    IDLE = "idle"
    HEATING = "heating"
    PURGE = "Purge"
    RETRACTING = "retracting"
    CLOSING_VALVE = "closing valve"
    HOME = "Home"


# Backend outwards
class MachineState:
    IDLE = "idle"
    PURGE = "purge"
    HOME = "home"
    BREWING = "brewing"
    ERROR = "error"  # so far unused


class ControlTypes:
    FLOW = "Flow"
    PRESSURE = "Pressure"
    PISTON = "Piston"
    POWER = "Power"


@dataclass
class ShotData:
    """Class respresenting a Datapoint of the machine in time, used to track a shot"""

    pressure: float = 0.0
    flow: float = 0.0
    weight: float = 0.0
    temperature: float = 20.0
    status: str = ""  # Represented as "name" over socket.io
    profile: str = ""
    time: int = -1
    state: str = ""
    is_extracting: bool = False

    main_controller_kind: ControlTypes = None  # {"Flow","Pressure","Piston","Power"}
    main_setpoint: float = -1
    aux_controller_kind: ControlTypes = None  # {"Flow","Pressure","Power"}
    aux_setpoint: float = -1
    is_aux_controller_active: bool = False

    def clone_with_time_and_state(self, shot_start_time, is_brewing):
        return replace(self, time=shot_start_time, is_extracting=is_brewing)

    def from_args(args):
        try:
            s = args[4].strip("\r\n")
            status = s
        except Exception:
            status = None

        try:
            profile = args[5].strip("\r\n")
        except Exception:
            profile = None

        main_controller_kind = None
        main_setpoint = 0.0
        aux_controller_kind = None
        aux_setpoint = 0.0
        is_aux_controller_active = False
        if len(args) > 10:
            try:
                main_controller_kind = args[6].strip("\r\n")
                if main_controller_kind == "none":
                    main_controller_kind = None

                main_setpoint = float(args[7].strip("\r\n"))
                aux_controller_kind = args[8].strip("\r\n")
                if aux_controller_kind == "none":
                    aux_controller_kind = None

                aux_setpoint = float(args[9].strip("\r\n"))
                is_aux_controller_active = args[10].strip("\r\n") == "true"
            except Exception as e:
                logger.warning(f"Failed to parse ShotData: {args}", exc_info=e)
                pass

        state = MachineState.IDLE
        if profile is not None:
            if profile not in [
                MachineStatus.IDLE,
                MachineStatus.PURGE,
                MachineStatus.HOME,
            ]:
                state = MachineState.BREWING
            else:
                state = profile.lower()

        try:
            data = ShotData(
                safe_float_with_nan(args[0]),
                safe_float_with_nan(args[1]),
                safe_float_with_nan(args[2]),
                safe_float_with_nan(args[3]),
                status,
                profile,
                state=state,
                main_controller_kind=main_controller_kind,
                main_setpoint=main_setpoint,
                aux_controller_kind=aux_controller_kind,
                aux_setpoint=aux_setpoint,
                is_aux_controller_active=is_aux_controller_active,
            )
        except Exception as e:
            logger.warning(f"Failed to parse ShotData: {args}", exc_info=e)
            return None

        return data

    def to_sio(self):
        setpoints = {}

        if self.main_controller_kind is not None:
            setpoints[self.main_controller_kind.lower()] = self.main_setpoint
            setpoints["active"] = self.main_controller_kind.lower()
        if self.aux_controller_kind is not None:
            setpoints[self.aux_controller_kind.lower()] = self.aux_setpoint
            if self.is_aux_controller_active:
                setpoints["active"] = self.aux_controller_kind.lower()

        data = {
            "name": self.status,
            "sensors": {
                "p": self.pressure,
                "f": self.flow,
                "w": self.weight,
                "t": self.temperature,
            },
            "setpoints": setpoints,
            "time": self.time,
            "profile": self.profile,
            "state": self.state,
            "extracting": self.is_extracting,
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
            "encoder_button_pressed": "ENCODER_PRESSED",
            "encoder_button_released": "ENCODER_RELEASED",
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

            event = ButtonEventData(
                ButtonEventEnum.from_str(args[0]), time_since_last_event
            )
        except Exception as e:
            logger.warning(f"Failed to parse EncoderEventData: {args}", exc_info=e)
            return None

        return event

    def to_sio(self):
        return {
            "type": self.event.name,
            "time_since_last_event": int(self.time_since_last_event),
        }


@dataclass
class MachineNotify:
    """Class respresenting a message the ESP wants the user to know"""

    notificationType: str = ""
    message: str = ""

    def from_args(args):
        try:
            notify = MachineNotify(args[0], args[1])
        except Exception as e:
            logger.warning(f"Failed to parse MachineNotify: {args}", exc_info=e)
            return None
        return notify


@dataclass
class HeaterTimeoutInfo:
    """Class representing heater timeout information received from the microcontroller"""

    # Time remaining for profile end timeout
    coffe_profile_end_remaining: float
    # Total timeout for profile end
    coffe_profile_end_timeout: float
    # Time remaining for preheat timeout
    preheat_remaining: float
    # Total timeout for preheat
    preheat_timeout: float

    @classmethod
    def from_args(cls, args):
        """
        Create a HeaterTimeoutInfo instance from a list of arguments.

        Args:
            args (list): List containing timeout information
                         [coffe_profile_end_remaining, coffe_profile_end_timeout,
                          preheat_remaining, preheat_timeout]

        Returns:
            HeaterTimeoutInfo: An instance of HeaterTimeoutInfo
        """
        if len(args) != 4:
            raise ValueError("Expected 4 arguments for HeaterTimeoutInfo")

        return cls(
            coffe_profile_end_remaining=float(args[0]),
            coffe_profile_end_timeout=float(args[1]),
            preheat_remaining=float(args[2]),
            preheat_timeout=float(args[3]),
        )

    def to_dict(self):
        """Convert the HeaterTimeoutInfo to a dictionary"""
        return {
            "coffe_profile_end": {
                "remaining": self.coffe_profile_end_remaining,
                "timeout": self.coffe_profile_end_timeout,
            },
            "preheat": {
                "remaining": self.preheat_remaining,
                "timeout": self.preheat_timeout,
            },
        }

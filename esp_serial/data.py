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
    motor_temp: float = 0.0
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
    weight_prediction: float = 0.0

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
            data = SensorData(
                external_1=safeFloat(args[0]),
                external_2=safeFloat(args[1]),
                bar_up=safeFloat(args[2]),
                bar_mid_up=safeFloat(args[3]),
                bar_mid_down=safeFloat(args[4]),
                bar_down=safeFloat(args[5]),
                tube=safeFloat(args[6]),
                motor_temp=safeFloat(args[7]),
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
                water_status=args[20].lower() == "true",
                motor_thermistor=safe_float_with_nan(args[21]),
                weight_prediction=safe_float_with_nan(args[22]),
            )

        except Exception as e:
            logger.warning(
                f"Failed to parse SensorData ({len(args)}): {args}", exc_info=e
            )
            return None
        return data

    def to_args(self):
        """Convert SensorData to a list of arguments for serial communication."""
        args = [
            str(self.external_1),
            str(self.external_2),
            str(self.bar_up),
            str(self.bar_mid_up),
            str(self.bar_mid_down),
            str(self.bar_down),
            str(self.tube),
            str(self.motor_temp),
            str(self.lam_temp),
            str(self.motor_position),
            str(self.motor_speed),
            str(self.motor_power),
            str(self.motor_current),
            str(self.bandheater_current),
            str(self.bandheater_power),
            str(self.pressure_sensor),
            str(self.adc_0),
            str(self.adc_1),
            str(self.adc_2),
            str(self.adc_3),
            "true" if self.water_status else "false",
            str(self.motor_thermistor),
            str(self.weight_prediction),
        ]
        return args

    def to_sio_sensors(self):
        return {
            "t_ext_1": self.external_1,
            "t_ext_2": self.external_2,
            "t_bar_up": self.bar_up,
            "t_bar_mu": self.bar_mid_up,
            "t_bar_md": self.bar_mid_down,
            "t_bar_down": self.bar_down,
            "t_tube": self.tube,
            "t_motor_temp": self.motor_temp,
            "lam_temp": self.lam_temp,
            "p": self.pressure_sensor,
            "a_0": self.adc_0,
            "a_1": self.adc_1,
            "a_2": self.adc_2,
            "a_3": self.adc_3,
            "m_pos": self.motor_position,
            "m_spd": self.motor_speed,
            "m_pwr": self.motor_power,
            "m_cur": self.motor_current,
            "bh_pwr": self.bandheater_power,
            "bh_cur": self.bandheater_current,
            "w_stat": self.water_status,
            "motor_temp": self.motor_thermistor,
            "weight_pred": self.weight_prediction,
        }


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
    scaleModule: str = ""

    def from_args(args):
        espPinout = 0
        try:
            # This used to be the fan status. To not break on old firmware we check parseability
            espPinout = int(args[1])
        except Exception:
            pass
        try:
            if len(args) >= 8:
                info = ESPInfo(
                    args[0],
                    espPinout,
                    float(args[2]),
                    args[3],
                    args[4],
                    args[5],
                    args[6],
                    args[7],
                )
            else:
                info = ESPInfo(args[0], espPinout, float(args[2]))
        except Exception as e:
            logger.warning(f"Failed to parse ESPInfo: {args}", exc_info=e)
            return None
        return info

    def to_args(self):
        """Convert ESPInfo to a list of arguments for serial communication."""
        args = [
            self.firmwareV,
            str(self.espPinout),
            str(self.mainVoltage),
            self.color,
            self.serialNumber,
            self.batchNumber,
            self.buildDate,
            self.scaleModule,
        ]
        return args

    def to_sio(self):
        """Convert ESPInfo to a dictionary for socket.io communication."""
        return {
            "firmware_version": self.firmwareV,
            "esp_pinout": self.espPinout,
            "main_voltage": self.mainVoltage,
            "color": self.color,
            "serial_number": self.serialNumber,
            "batch_number": self.batchNumber,
            "build_date": self.buildDate,
            "scale_module": self.scaleModule,
        }


@dataclass
class ESPTasks:

    tasks: dict[str, int]

    def from_args(args: list[str]):
        tasks: dict[str, int] = {}
        try:
            for arg in args:
                task_data = arg.split(":")
                tasks[str(task_data[0])] = int(task_data[1])

            return ESPTasks(tasks=tasks)

        except Exception as e:
            logger.warning(f"Failed to parse ESP tasks info: {e}")
            return None


# From ESP32 to backend
class MachineStatus:
    # Enum representing the events from the machine
    IDLE = "idle"
    HEATING = "heating"
    PURGE = "purge"
    RETRACTING = "retracting"
    CLOSING_VALVE = "closing valve"
    HOME = "home"
    BOOT = "boot"
    STARTING = "starting..."


MachineStatusToProfile = {
    MachineStatus.PURGE: "Purge",
    MachineStatus.HOME: "Home",
    MachineStatus.BOOT: "Bootup",
}


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
    stable_weight: bool = False
    temperature: float = 20.0
    status: str = ""  # Represented as "name" over socket.io
    profile: str = ""
    time: int = -1
    profile_time: int = -1
    state: str = ""
    is_extracting: bool = False
    gravimetric_flow: float = 0.0

    main_controller_kind: ControlTypes = (
        None  # {"Flow","Pressure","Piston","Power", "Temperature"}
    )
    main_setpoint: float = -1
    aux_controller_kind: ControlTypes = None  # {"Flow","Pressure","Power"}
    aux_setpoint: float = -1
    is_aux_controller_active: bool = False

    def clone_with_time_and_state(self, shot_start_time, is_brewing, profile_time=-1):
        return replace(
            self,
            time=shot_start_time,
            is_extracting=is_brewing,
            profile_time=profile_time,
        )

    def to_args(self):
        """Convert ShotData to a list of arguments for serial communication."""
        args = [
            str(self.pressure),
            str(self.flow),
            str(self.weight),
            "S" if self.stable_weight else "U",
            str(self.temperature),
            self.status or "",
            self.profile or "",
        ]

        if self.main_controller_kind is not None:
            args.append(self.main_controller_kind)
            args.append(str(self.main_setpoint))
        else:
            args.append("none")
            args.append("0.0")

        if self.aux_controller_kind is not None:
            args.append(self.aux_controller_kind)
            args.append(str(self.aux_setpoint))
            args.append("true" if self.is_aux_controller_active else "false")
        else:
            args.append("none")
            args.append("0.0")
            args.append("false")

        args.append(str(self.gravimetric_flow))

        return args

    def from_args(args):
        try:
            s = args[5].strip("\r\n")
            status = s
        except Exception:
            status = None

        try:
            profile = args[6].strip("\r\n")
        except Exception:
            profile = None

        main_controller_kind = None
        main_setpoint = 0.0
        aux_controller_kind = None
        aux_setpoint = 0.0
        is_aux_controller_active = False
        gravimetric_flow = 0.0
        stable_weight = args[3].strip("\r\n") == "S"

        if len(args) > 12:
            try:
                main_controller_kind = args[7].strip("\r\n")
                if main_controller_kind == "none":
                    main_controller_kind = None

                main_setpoint = float(args[8].strip("\r\n"))
                aux_controller_kind = args[9].strip("\r\n")
                if aux_controller_kind == "none":
                    aux_controller_kind = None

                aux_setpoint = float(args[10].strip("\r\n"))
                is_aux_controller_active = args[11].strip("\r\n") == "true"
                gravimetric_flow = safe_float_with_nan(args[12])
            except Exception as e:
                logger.warning(f"Failed to parse ShotData: {args}", exc_info=e)
                pass

        state = MachineState.IDLE
        if profile is not None:
            if profile not in [
                MachineStatus.IDLE,
                MachineStatusToProfile[MachineStatus.PURGE],
                MachineStatusToProfile[MachineStatus.HOME],
            ]:
                state = MachineState.BREWING
            else:
                state = profile.lower()

        try:
            data = ShotData(
                safe_float_with_nan(args[0]),
                safe_float_with_nan(args[1]),
                safe_float_with_nan(args[2]),
                stable_weight,
                safe_float_with_nan(args[4]),
                status,
                profile,
                state=state,
                main_controller_kind=main_controller_kind,
                main_setpoint=main_setpoint,
                aux_controller_kind=aux_controller_kind,
                aux_setpoint=aux_setpoint,
                is_aux_controller_active=is_aux_controller_active,
                gravimetric_flow=gravimetric_flow,
            )
        except Exception as e:
            logger.warning(f"Failed to parse ShotData: {args}", exc_info=e)
            return None

        return data

    def to_sio(self):
        setpoints = {
            "active": None,
        }

        if self.main_controller_kind is not None:
            setpoints[self.main_controller_kind.lower()] = self.main_setpoint
            setpoints["active"] = self.main_controller_kind.lower()
        if self.aux_controller_kind is not None:
            setpoints[self.aux_controller_kind.lower()] = self.aux_setpoint
            if self.is_aux_controller_active:
                setpoints["active"] = self.aux_controller_kind.lower()

        # Create sensors dictionary with base data
        sensors = {
            "p": self.pressure,
            "f": self.flow,
            "w": self.weight,
            "t": self.temperature,
            "g": self.gravimetric_flow,
        }

        data = {
            "name": self.status,
            "sensors": sensors,
            "setpoints": setpoints,
            "time": self.time,
            "profile": self.profile,
            "profile_time": self.profile_time,
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
    TARE_PRESSED = auto()
    TARE_RELEASED = auto()
    CONTEXT_PRESSED = auto()
    CONTEXT_RELEASED = auto()

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
            "tare_pressed": "TARE_PRESSED",
            "tare_released": "TARE_RELEASED",
            "context_pressed": "CONTEXT_PRESSED",
            "context_released": "CONTEXT_RELEASED",
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

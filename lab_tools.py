from dataclasses import dataclass
from typing import Literal

from log import MeticulousLogger
from machine import Machine
from profiles import ProfileManager

logger = MeticulousLogger.getLogger(__name__)

MotorMode = Literal["up", "down", "ramp"]


@dataclass
class MotorHeaterRequest:
    motor_power: int
    band_heater_power: int
    motor_mode: MotorMode


class LabTools:
    active_profile_id = None

    @staticmethod
    def _clamp_power(value) -> int:
        try:
            numeric_value = int(value)
        except (TypeError, ValueError):
            raise ValueError(f"Invalid power value: {value}")
        return min(max(numeric_value, 0), 100)

    @staticmethod
    def parse_motor_heater_request(data) -> MotorHeaterRequest:
        motor_mode = data.get("motor_mode", "up")
        if motor_mode not in ["up", "down", "ramp"]:
            raise ValueError("motor_mode must be one of: up, down, ramp")

        return MotorHeaterRequest(
            motor_power=LabTools._clamp_power(data.get("motor_power", 0)),
            band_heater_power=LabTools._clamp_power(data.get("band_heater_power", 0)),
            motor_mode=motor_mode,
        )

    @staticmethod
    def _power_profile(request: MotorHeaterRequest):
        motor_power = -request.motor_power if request.motor_mode == "up" else request.motor_power
        return {
            "name": "Lab Motor and Band Heater Control",
            "id": "lab_motor_band_heater_control",
            "source": "Meticulous",
            "stages": [
                {
                    "name": "lab_control",
                    "nodes": [
                        {
                            "id": -1,
                            "controllers": [{"kind": "time_reference", "id": 1}],
                            "triggers": [{"kind": "exit", "next_node_id": 1}],
                        },
                        {
                            "id": 1,
                            "controllers": [
                                {
                                    "kind": "piston_power_controller",
                                    "algorithm": "Spring v1.0",
                                    "curve": {
                                        "id": 1,
                                        "interpolation_kind": "linear_interpolation",
                                        "points": [[0, motor_power]],
                                        "time_reference_id": 1,
                                    },
                                },
                                {
                                    "kind": "heater_power_controller",
                                    "algorithm": "Heater Power Bypass",
                                    "curve": {
                                        "id": 2,
                                        "interpolation_kind": "linear_interpolation",
                                        "points": [[0, request.band_heater_power]],
                                        "time_reference_id": 1,
                                    },
                                },
                            ],
                            "triggers": [],
                        },
                        {"id": -2, "controllers": [{"kind": "end_profile"}], "triggers": []},
                    ],
                }
            ],
        }

    @staticmethod
    def _ramp_profile(request: MotorHeaterRequest):
        return {
            "name": "Lab Motor Ramp",
            "id": "lab_motor_ramp",
            "source": "Meticulous",
            "stages": [
                {
                    "name": "lab_motor_ramp",
                    "nodes": [
                        {"id": -1, "controllers": [], "triggers": [{"kind": "exit", "next_node_id": 100}]},
                        {
                            "id": 100,
                            "controllers": [{"kind": "time_reference", "id": 1}],
                            "triggers": [{"kind": "exit", "next_node_id": 1}],
                        },
                        {
                            "id": 1,
                            "controllers": [
                                {
                                    "kind": "heater_power_controller",
                                    "algorithm": "Heater Power Bypass",
                                    "curve": {
                                        "id": 201,
                                        "interpolation_kind": "linear_interpolation",
                                        "points": [[0, request.band_heater_power]],
                                        "time_reference_id": 1,
                                    },
                                }
                            ],
                            "triggers": [{"kind": "timer_trigger", "timer_reference_id": 1, "operator": ">=", "value": 1, "next_node_id": 200}],
                        },
                        {
                            "id": 200,
                            "controllers": [{"kind": "time_reference", "id": 2}],
                            "triggers": [{"kind": "exit", "next_node_id": 2}],
                        },
                        {
                            "id": 2,
                            "controllers": [LabTools._motor_controller(101, -request.motor_power, 2)],
                            "triggers": [{"kind": "timer_trigger", "timer_reference_id": 2, "operator": ">=", "value": 1, "next_node_id": 300}],
                        },
                        {
                            "id": 300,
                            "controllers": [{"kind": "time_reference", "id": 3}],
                            "triggers": [{"kind": "exit", "next_node_id": 3}],
                        },
                        {
                            "id": 3,
                            "controllers": [LabTools._motor_controller(102, -request.motor_power, 3)],
                            "triggers": [
                                {"kind": "piston_speed_trigger", "operator": "==", "value": 0, "next_node_id": 400},
                                {"kind": "timer_trigger", "timer_reference_id": 3, "operator": ">=", "value": 300, "next_node_id": 400},
                            ],
                        },
                        {
                            "id": 400,
                            "controllers": [{"kind": "time_reference", "id": 4}],
                            "triggers": [{"kind": "exit", "next_node_id": 4}],
                        },
                        {
                            "id": 4,
                            "controllers": [LabTools._motor_controller(103, request.motor_power, 4)],
                            "triggers": [{"kind": "timer_trigger", "timer_reference_id": 4, "operator": ">=", "value": 1, "next_node_id": 500}],
                        },
                        {
                            "id": 500,
                            "controllers": [{"kind": "time_reference", "id": 5}],
                            "triggers": [{"kind": "exit", "next_node_id": 5}],
                        },
                        {
                            "id": 5,
                            "controllers": [LabTools._motor_controller(104, request.motor_power, 5)],
                            "triggers": [
                                {"kind": "piston_speed_trigger", "operator": "==", "value": 0, "next_node_id": 100},
                                {"kind": "timer_trigger", "timer_reference_id": 5, "operator": ">=", "value": 300, "next_node_id": 100},
                            ],
                        },
                        {"id": -2, "controllers": [{"kind": "end_profile"}], "triggers": []},
                    ],
                }
            ],
        }

    @staticmethod
    def _motor_controller(curve_id: int, power: int, time_reference_id: int):
        return {
            "kind": "piston_power_controller",
            "algorithm": "Spring v1.0",
            "curve": {
                "id": curve_id,
                "interpolation_kind": "linear_interpolation",
                "points": [[0, power]],
                "time_reference_id": time_reference_id,
            },
        }

    @staticmethod
    def start_motor_heater(request: MotorHeaterRequest):
        if not Machine.is_idle:
            raise RuntimeError("machine is busy")

        profile = (
            LabTools._ramp_profile(request)
            if request.motor_mode == "ramp"
            else LabTools._power_profile(request)
        )

        logger.warning(
            "Starting lab motor/heater control: "
            + f"motor_power={request.motor_power}, "
            + f"band_heater_power={request.band_heater_power}, "
            + f"motor_mode={request.motor_mode}"
        )
        ProfileManager._set_last_profile(profile)
        Machine.send_json_with_hash(profile)
        Machine.action("start")
        LabTools.active_profile_id = profile["id"]
        return profile

    @staticmethod
    def stop():
        logger.warning("Stopping lab motor/heater control")
        Machine.action("stop")
        LabTools.active_profile_id = None

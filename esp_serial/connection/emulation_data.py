from ..data import ShotData, SensorData, ESPInfo
import json
import os


class EmulationData:
    PROFILE_PLACEHOLDER = "$$PROFILE_PLACEHOLDER$$"
    IDLE_DATA = []
    ESPRESSO_DATA = []

    def shotToEmulation(shot, isProfile=False):
        emulationData = []
        shot_data = shot["data"]
        for sample in shot_data:
            sensor = sample["sensors"]
            data = sample["shot"]
            if sample["time"] <= 0:
                continue

            new_sensors = SensorData(
                external_1=sensor["external_1"],
                external_2=sensor["external_2"],
                bar_up=sensor["bar_up"],
                bar_mid_up=sensor["bar_mid_up"],
                bar_mid_down=sensor["bar_mid_down"],
                bar_down=sensor["bar_down"],
                tube=sensor["tube"],
                motor_temp=sensor["motor_temp"],
                lam_temp=sensor["lam_temp"],
                motor_position=sensor["motor_position"],
                motor_speed=sensor["motor_speed"],
                motor_power=sensor["motor_power"],
                motor_current=sensor["motor_current"],
                bandheater_power=sensor["bandheater_power"],
                bandheater_current=sensor["bandheater_current"],
                pressure_sensor=sensor["pressure_sensor"],
                adc_0=sensor["adc_0"],
                adc_1=sensor["adc_1"],
                adc_2=sensor["adc_2"],
                adc_3=sensor["adc_3"],
                water_status=sensor.get("water_status", False),
            )
            setpoints_type = data["setpoints"]["active"]
            main_setpoint = None

            if setpoints_type is not None:
                main_setpoint = data["setpoints"].get(setpoints_type, None)

            new_shot = ShotData(
                pressure=data["pressure"],
                flow=data["flow"],
                weight=data["weight"],
                gravimetric_flow=data["gravimetric_flow"],
                temperature=data.get("temperature", sensor["tube"]),
                profile=(
                    EmulationData.PROFILE_PLACEHOLDER
                    if isProfile
                    else shot["profile_name"]
                ),
                status=sample["status"],
                main_controller_kind=data["setpoints"]["active"],
                main_setpoint=main_setpoint,
                # aux_controller_kind=data["setpoints"].get("aux", None),
                # aux_setpoint=aux_setpoint,
            )
            emulationData.append("Data," + ",".join(new_shot.to_args()))
            emulationData.append("Sensors," + ",".join(new_sensors.to_args()))
        return emulationData

    def init():
        """
        Initialize the emulation data with default values.
        This method is called to set up the initial state of the emulation data.
        """
        idleDataArgs = ShotData(
            temperature=23.2, state="idle", status="idle", profile="idle"
        ).to_args()
        idleSensorsArgs = SensorData(
            external_1=85.11,
            external_2=86.27,
            bar_up=68.73,
            bar_mid_up=69.02,
            bar_mid_down=67.75,
            bar_down=65.44,
            tube=67.74,
            motor_temp=29.96,
            lam_temp=32.78,
            motor_position=74.0,
            motor_speed=0.0,
            motor_power=0.0,
            motor_current=0.0,
            bandheater_power=15.3,
            bandheater_current=0.62,
            pressure_sensor=306.0,
            adc_0=14.0,
            adc_1=14.0,
            adc_2=14.0,
            adc_3=14.0,
            water_status=False,
            motor_thermistor=0.0,
        ).to_args()

        EmulationData.IDLE_DATA = [
            "Data," + ",".join(idleDataArgs),
            "Sensors," + ",".join(idleSensorsArgs),
            "ESPInfo," + ",".join(ESPInfo().to_args()),
        ]

        emulated_shot_path = os.path.join(
            os.path.dirname(__file__), "emulated.shot.json"
        )
        with open(emulated_shot_path, "r") as f:
            shot = json.load(f)
            EmulationData.ESPRESSO_DATA = EmulationData.shotToEmulation(shot)

        emulated_home_path = os.path.join(
            os.path.dirname(__file__), "emulated.home.json"
        )
        with open(emulated_home_path, "r") as f:
            shot = json.load(f)
            EmulationData.HOME_DATA = EmulationData.shotToEmulation(shot)

        emulated_purge_path = os.path.join(
            os.path.dirname(__file__), "emulated.purge.json"
        )
        with open(emulated_purge_path, "r") as f:
            shot = json.load(f)
            EmulationData.PURGE_DATA = EmulationData.shotToEmulation(shot)

        print("EmulationData initialized")

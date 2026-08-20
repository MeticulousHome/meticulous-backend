import pytest

from esp_serial.data import (
    SensorData,
    ShotData,
    ESPInfo,
    ButtonEventData,
    ButtonEventEnum,
    HeaterTimeoutInfo,
    MachineNotify,
    MachineState,
    TestRAData,
    TestRASchema,
    safeFloat,
    safe_float_with_nan,
)


class TestSafeFloat:
    def test_normal_value(self):
        assert safeFloat("3.14") == 3.14

    def test_zero(self):
        assert safeFloat("0") == 0.0

    def test_negative(self):
        assert safeFloat("-1.5") == -1.5

    def test_nan_returns_zero(self):
        assert safeFloat("nan") == 0

    def test_integer_string(self):
        assert safeFloat("42") == 42.0

    def test_invalid_raises(self):
        with pytest.raises(ValueError):
            safeFloat("notanumber")


class TestSafeFloatWithNan:
    def test_normal_value(self):
        assert safe_float_with_nan("3.14") == 3.14

    def test_nan_returns_string(self):
        assert safe_float_with_nan("nan") == "NaN"

    def test_invalid_returns_nan_string(self):
        assert safe_float_with_nan("notanumber") == "NaN"

    def test_zero(self):
        assert safe_float_with_nan("0") == 0.0

    def test_negative(self):
        assert safe_float_with_nan("-2.5") == -2.5


class TestSensorData:
    def make_args(self, **overrides):
        defaults = [
            "90.1",  # external_1
            "91.2",  # external_2
            "92.0",  # bar_up
            "93.0",  # bar_mid_up
            "94.0",  # bar_mid_down
            "95.0",  # bar_down
            "85.0",  # tube
            "40.0",  # motor_temp
            "50.0",  # lam_temp
            "10.5",  # motor_position
            "100.0",  # motor_speed
            "5.0",  # motor_power
            "2.5",  # motor_current
            "1.0",  # bandheater_current
            "3.0",  # bandheater_power
            "9.0",  # pressure_sensor
            "0.1",  # adc_0
            "0.2",  # adc_1
            "0.3",  # adc_2
            "0.4",  # adc_3
            "true",  # water_status
            "35.0",  # motor_thermistor
            "18.5",  # weight_prediction
        ]
        return defaults

    def test_parse_all_fields(self):
        args = self.make_args()
        data = SensorData.from_args(args)
        assert data is not None
        assert data.external_1 == 90.1
        assert data.external_2 == 91.2
        assert data.bar_up == 92.0
        assert data.pressure_sensor == 9.0
        assert data.water_status is True
        assert data.motor_thermistor == 35.0
        assert data.weight_prediction == 18.5

    def test_water_status_false(self):
        args = self.make_args()
        args[20] = "false"
        data = SensorData.from_args(args)
        assert data.water_status is False

    def test_nan_motor_thermistor(self):
        args = self.make_args()
        args[21] = "nan"
        data = SensorData.from_args(args)
        assert data.motor_thermistor == "NaN"

    def test_too_few_args_returns_none(self):
        data = SensorData.from_args(["1.0", "2.0"])
        assert data is None

    def test_roundtrip_to_args(self):
        args = self.make_args()
        data = SensorData.from_args(args)
        output = data.to_args()
        reparsed = SensorData.from_args(output)
        assert reparsed.external_1 == data.external_1
        assert reparsed.pressure_sensor == data.pressure_sensor
        assert reparsed.water_status == data.water_status

    def test_to_sio_sensors_keys(self):
        args = self.make_args()
        data = SensorData.from_args(args)
        sio = data.to_sio_sensors()
        assert "p" in sio
        assert "t_ext_1" in sio
        assert "w_stat" in sio
        assert sio["p"] == 9.0


class TestShotData:
    def make_args(self, with_controllers=True):
        base = [
            "9.0",  # pressure
            "2.5",  # flow
            "18.0",  # weight
            "S",  # stable_weight
            "93.5",  # temperature
            "idle",  # status
            "idle",  # profile
        ]
        if with_controllers:
            base += [
                "Pressure",  # main_controller_kind
                "9.0",  # main_setpoint
                "Flow",  # aux_controller_kind
                "2.0",  # aux_setpoint
                "true",  # is_aux_controller_active
                "1.5",  # gravimetric_flow
            ]
        return base

    def test_parse_with_controllers(self):
        args = self.make_args(with_controllers=True)
        data = ShotData.from_args(args)
        assert data is not None
        assert data.pressure == 9.0
        assert data.flow == 2.5
        assert data.weight == 18.0
        assert data.stable_weight is True
        assert data.temperature == 93.5
        assert data.status == "idle"
        assert data.profile == "idle"
        assert data.main_controller_kind == "Pressure"
        assert data.main_setpoint == 9.0
        assert data.aux_controller_kind == "Flow"
        assert data.aux_setpoint == 2.0
        assert data.is_aux_controller_active is True
        assert data.gravimetric_flow == 1.5

    def test_parse_without_controllers(self):
        args = self.make_args(with_controllers=False)
        data = ShotData.from_args(args)
        assert data is not None
        assert data.main_controller_kind is None
        assert data.aux_controller_kind is None
        assert data.gravimetric_flow == 0.0

    def test_unstable_weight(self):
        args = self.make_args()
        args[3] = "U"
        data = ShotData.from_args(args)
        assert data.stable_weight is False

    def test_brewing_state(self):
        args = self.make_args()
        args[6] = "MyEspresso"
        data = ShotData.from_args(args)
        assert data.state == MachineState.BREWING

    def test_purge_state(self):
        args = self.make_args()
        args[6] = "Purge"
        data = ShotData.from_args(args)
        assert data.state == "purge"

    def test_home_state(self):
        args = self.make_args()
        args[6] = "Home"
        data = ShotData.from_args(args)
        assert data.state == "home"

    def test_nan_pressure(self):
        args = self.make_args()
        args[0] = "nan"
        data = ShotData.from_args(args)
        assert data.pressure == "NaN"

    def test_to_sio_structure(self):
        args = self.make_args(with_controllers=True)
        data = ShotData.from_args(args)
        sio = data.to_sio()
        assert "sensors" in sio
        assert "setpoints" in sio
        assert sio["sensors"]["p"] == 9.0
        assert sio["sensors"]["f"] == 2.5
        assert sio["setpoints"]["active"] == "flow"
        assert sio["setpoints"]["pressure"] == 9.0

    def test_roundtrip_to_args(self):
        args = self.make_args(with_controllers=True)
        data = ShotData.from_args(args)
        output = data.to_args()
        reparsed = ShotData.from_args(output)
        assert reparsed.pressure == data.pressure
        assert reparsed.flow == data.flow
        assert reparsed.main_controller_kind == data.main_controller_kind


class TestESPInfo:
    def test_parse_full(self):
        args = ["1.2.3", "2", "24.5", "black", "SN123", "B456", "2024-01-01", "scale1"]
        info = ESPInfo.from_args(args)
        assert info is not None
        assert info.firmwareV == "1.2.3"
        assert info.espPinout == 2
        assert info.mainVoltage == 24.5
        assert info.color == "black"
        assert info.serialNumber == "SN123"
        assert info.batchNumber == "B456"
        assert info.buildDate == "2024-01-01"
        assert info.scaleModule == "scale1"

    def test_parse_minimal(self):
        args = ["0.9.1", "1", "23.0"]
        info = ESPInfo.from_args(args)
        assert info is not None
        assert info.firmwareV == "0.9.1"
        assert info.espPinout == 1
        assert info.mainVoltage == 23.0
        assert info.color == ""
        assert info.serialNumber == ""

    def test_invalid_pinout_defaults_zero(self):
        args = ["1.0.0", "notanint", "24.0", "", "", "", "", ""]
        info = ESPInfo.from_args(args)
        assert info is not None
        assert info.espPinout == 0

    def test_to_sio(self):
        args = ["1.2.3", "2", "24.5", "black", "SN123", "B456", "2024-01-01", "scale1"]
        info = ESPInfo.from_args(args)
        sio = info.to_sio()
        assert sio["firmware_version"] == "1.2.3"
        assert sio["esp_pinout"] == 2
        assert sio["serial_number"] == "SN123"

    def test_roundtrip_to_args(self):
        args = ["1.2.3", "2", "24.5", "black", "SN123", "B456", "2024-01-01", "scale1"]
        info = ESPInfo.from_args(args)
        output = info.to_args()
        reparsed = ESPInfo.from_args(output)
        assert reparsed.firmwareV == info.firmwareV
        assert reparsed.mainVoltage == info.mainVoltage
        assert reparsed.color == info.color


class TestButtonEventData:
    def test_parse_encoder_clockwise(self):
        event = ButtonEventData.from_args(["CW", "150"])
        assert event is not None
        assert event.event == ButtonEventEnum.ENCODER_CLOCKWISE
        assert event.time_since_last_event == 150

    def test_parse_tare(self):
        event = ButtonEventData.from_args(["tare", "500"])
        assert event.event == ButtonEventEnum.TARE

    def test_parse_context(self):
        event = ButtonEventData.from_args(["strt", "0"])
        assert event.event == ButtonEventEnum.CONTEXT

    def test_parse_cntx_alias(self):
        event = ButtonEventData.from_args(["cntx", "100"])
        assert event.event == ButtonEventEnum.CONTEXT

    def test_overflow_time(self):
        event = ButtonEventData.from_args(["push", "9999+++"])
        assert event.time_since_last_event == 10000

    def test_no_time_arg(self):
        event = ButtonEventData.from_args(["push"])
        assert event.event == ButtonEventEnum.ENCODER_PUSH
        assert event.time_since_last_event == 0

    def test_unknown_event_returns_none(self):
        event = ButtonEventData.from_args(["UNKNOWN_EVENT", "0"])
        assert event is None

    def test_button_pressed_released(self):
        pressed = ButtonEventData.from_args(["encoder_button_pressed", "0"])
        released = ButtonEventData.from_args(["encoder_button_released", "50"])
        assert pressed.event == ButtonEventEnum.ENCODER_PRESSED
        assert released.event == ButtonEventEnum.ENCODER_RELEASED

    def test_to_sio(self):
        event = ButtonEventData.from_args(["tare", "200"])
        sio = event.to_sio()
        assert sio["type"] == "TARE"
        assert sio["time_since_last_event"] == 200


class TestHeaterTimeoutInfo:
    def test_parse_valid(self):
        info = HeaterTimeoutInfo.from_args(["300.0", "600.0", "120.0", "180.0"])
        assert info.coffe_profile_end_remaining == 300.0
        assert info.coffe_profile_end_timeout == 600.0
        assert info.preheat_remaining == 120.0
        assert info.preheat_timeout == 180.0

    def test_wrong_arg_count_raises(self):
        with pytest.raises(ValueError):
            HeaterTimeoutInfo.from_args(["300.0", "600.0"])

    def test_to_dict(self):
        info = HeaterTimeoutInfo.from_args(["300.0", "600.0", "120.0", "180.0"])
        d = info.to_dict()
        assert d["coffe_profile_end"]["remaining"] == 300.0
        assert d["coffe_profile_end"]["timeout"] == 600.0
        assert d["preheat"]["remaining"] == 120.0
        assert d["preheat"]["timeout"] == 180.0


class TestMachineNotify:
    def test_parse_valid(self):
        notify = MachineNotify.from_args(["warning", "Low water"])
        assert notify.notificationType == "warning"
        assert notify.message == "Low water"

    def test_parse_empty_args_returns_none(self):
        notify = MachineNotify.from_args([])
        assert notify is None


SCHEMA_ARGS = [
    "1",
    "pressure",
    "water_temp",
    "ext_temp",
    "ext_temp_1",
    "ext_temp_2",
    "lam_temp",
    "tube_temp",
    "plunger_temp",
    "motor_temp",
]

NAMES = TestRASchema.from_args(SCHEMA_ARGS).names

# These are named after the TestRA wire prefix, not after pytest. Without this,
# pytest tries to collect them as test classes and warns on every run.
TestRAData.__test__ = False
TestRASchema.__test__ = False


def build_testra_args(entries, seq=1, uptime=12345):
    """Build a TestRA argument list. `entries` is a list of
    (raw, og, safe, echo, coherent) tuples, one per averager."""
    fields = []
    for raw, og, safe, echo, coherent in entries:
        fields += [str(raw), str(og), str(safe), str(echo), str(coherent)]
    # The firmware terminates every field with a comma, leaving a trailing
    # empty element after the split.
    return ["1", str(seq), str(uptime)] + fields + [""]


CLEAN_ROW = [(1.0, 9.0, 9.0, "nan", 1)] * 9


class TestRASchemaParsing:
    def test_names_and_version(self):
        schema = TestRASchema.from_args(SCHEMA_ARGS)
        assert schema.version == 1
        assert schema.names[0] == "pressure"
        assert schema.names[-1] == "motor_temp"
        assert len(schema.names) == 9

    def test_requires_version(self):
        with pytest.raises(ValueError):
            TestRASchema.from_args([])


class TestRAParsing:
    def test_parses_all_entries(self):
        data = TestRAData.from_args(build_testra_args(CLEAN_ROW), NAMES)
        assert data.seq == 1
        assert data.uptime_ms == 12345
        assert len(data.entries) == 9
        assert [e.name for e in data.entries] == NAMES

    def test_falls_back_to_positional_names_without_schema(self):
        data = TestRAData.from_args(build_testra_args(CLEAN_ROW), None)
        assert data.entries[0].name == "idx_0"

    def test_rejects_truncated_row(self):
        with pytest.raises(ValueError):
            TestRAData.from_args(["1", "2"], NAMES)

    def test_rejects_row_without_entries(self):
        with pytest.raises(ValueError):
            TestRAData.from_args(["1", "2", "3"], NAMES)


class TestRADivergence:
    def test_matching_windows_do_not_diverge(self):
        data = TestRAData.from_args(build_testra_args(CLEAN_ROW), NAMES)
        assert data.diverged_entries() == []

    def test_race_is_detected(self):
        rows = list(CLEAN_ROW)
        # The defect's signature: the original reads roughly 2x the true value.
        rows[6] = (93.5, 187.02, 93.51, 93.51, 1)
        data = TestRAData.from_args(build_testra_args(rows), NAMES)
        diverged = data.diverged_entries()
        assert len(diverged) == 1
        assert diverged[0].name == "tube_temp"
        assert diverged[0].divergence == pytest.approx(93.51, abs=1e-6)
        assert diverged[0].divergence_pct == pytest.approx(100.0, abs=1e-6)

    def test_incoherent_read_is_not_counted(self):
        rows = list(CLEAN_ROW)
        # Same divergence, but the window moved mid-read, so og and safe cover
        # different sample sets and the difference proves nothing.
        rows[6] = (93.5, 187.02, 93.51, 93.51, 0)
        data = TestRAData.from_args(build_testra_args(rows), NAMES)
        assert data.diverged_entries() == []

    def test_zero_mirror_reports_infinite_relative_error(self):
        rows = list(CLEAN_ROW)
        rows[1] = (0.0, 4.2, 0.0, "nan", 1)
        data = TestRAData.from_args(build_testra_args(rows), NAMES)
        entry = data.diverged_entries()[0]
        assert entry.divergence_pct == float("inf")

    def test_nan_average_is_not_a_divergence(self):
        rows = list(CLEAN_ROW)
        rows[0] = ("nan", "nan", "nan", "nan", 1)
        data = TestRAData.from_args(build_testra_args(rows), NAMES)
        assert data.diverged_entries() == []


class TestRAPublishCrossCheck:
    def sensor(self):
        return SensorData(
            external_1=20.0,
            external_2=21.0,
            tube=93.51,
            motor_temp=40.0,
            lam_temp=55.0,
        )

    def test_matching_echo_is_silent(self):
        rows = list(CLEAN_ROW)
        rows[3] = (20.0, 20.0, 20.0, 20.0, 1)
        data = TestRAData.from_args(build_testra_args(rows), NAMES)
        assert data.publish_mismatches(self.sensor()) == []

    def test_two_decimal_quantisation_is_tolerated(self):
        rows = list(CLEAN_ROW)
        # TestRA prints 4 decimals, "Sensors," prints 2. That gap must not
        # register as a mismatch.
        rows[6] = (93.5, 93.51, 93.51, 93.5137, 1)
        data = TestRAData.from_args(build_testra_args(rows), NAMES)
        assert data.publish_mismatches(self.sensor()) == []

    def test_field_desync_is_caught(self):
        rows = list(CLEAN_ROW)
        rows[8] = (40.0, 40.0, 40.0, 47.9, 1)
        data = TestRAData.from_args(build_testra_args(rows), NAMES)
        mismatches = data.publish_mismatches(self.sensor())
        assert len(mismatches) == 1
        assert mismatches[0]["name"] == "motor_temp"
        assert mismatches[0]["sensors_field"] == "motor_temp"

    def test_averager_without_counterpart_is_skipped(self):
        # Pressure is averaged a second time before it reaches "Data,", so it
        # carries a nan echo and must never be cross-checked.
        rows = list(CLEAN_ROW)
        rows[0] = (5.0, 5.0, 5.0, "nan", 1)
        data = TestRAData.from_args(build_testra_args(rows), NAMES)
        assert data.publish_mismatches(self.sensor()) == []

    def test_no_sensor_data_yet(self):
        data = TestRAData.from_args(build_testra_args(CLEAN_ROW), NAMES)
        assert data.publish_mismatches(None) == []

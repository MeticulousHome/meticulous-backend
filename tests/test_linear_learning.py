"""Tests for the main-board learner of LinearLearning, the ESP32 final-weight predictor."""

import hashlib
import json
import math
import os
from types import SimpleNamespace

import pytest
import zstandard

from linear_learning import (
    CalibrationStore,
    FleetModel,
    LinearLearningCalibrator,
    PostDecisionTrace,
    bucket_key,
    fit,
    replay,
)
from linear_learning import calibration as C
from linear_learning.model import PARAMS_PATH

FIXTURE = os.path.join(
    os.path.dirname(__file__), "fixtures", "linear_learning", "shot_10.10.0.42_milky.json.zst"
)


def load_fixture():
    with open(FIXTURE, "rb") as f:
        return json.loads(zstandard.ZstdDecompressor().stream_reader(f).read())


# ---------------- fleet model ----------------
def test_model_id_is_the_hash_of_the_fleet_constants():
    # The ESP32 firmware is built with the same constants and id; it only accepts a calibration
    # sent with its own id, so the id must change whenever a constant does.
    with open(PARAMS_PATH) as f:
        p = json.load(f)
    keys = (
        "names",
        "med",
        "mu",
        "sd",
        "coef",
        "b0",
        "key_names",
        "key_idx",
        "k_mu",
        "k_sd",
        "clip_fleet_sd",
        "clip_calib_sd",
    )
    canon = json.dumps({k: p[k] for k in keys}, sort_keys=True, separators=(",", ":"))
    assert p["model_id"] == "ll-" + hashlib.sha256(canon.encode()).hexdigest()[:8]
    assert FleetModel().model_id == p["model_id"]


def test_fleet_model_shape():
    with open(PARAMS_PATH) as f:
        p = json.load(f)
    assert all(len(p[k]) == 48 for k in ("names", "med", "mu", "sd", "coef"))
    assert [p["names"][i] for i in p["key_idx"]] == p["key_names"]
    assert FleetModel().default_cal == [0.0] * 9


# ---------------- calibration fit ----------------
def test_fit_offset_only_is_shrunk_recency_weighted_mean():
    records = [([0.0] * 8, 0.5)] * 5
    w = sum(math.exp(-(4 - i) / C.TAU) for i in range(5))
    cal = fit(records)
    assert cal[0] == pytest.approx(0.5 * w / (w + C.LAM_OFFSET), abs=1e-9)
    assert all(abs(c) < 1e-9 for c in cal[1:])


def test_fit_caps_residuals():
    big = fit([([0.0] * 8, 50.0)] * 3)
    capped = fit([([0.0] * 8, C.CLIP_G)] * 3)
    assert big[0] == pytest.approx(capped[0])


def test_bucket_key():
    assert bucket_key(45.33) == "45"
    assert bucket_key(67.99) == "68"
    assert bucket_key(None) == "unknown"


# ---------------- garbage gate + real changes ----------------
def _feed(store, residuals, bucket="45"):
    out = []
    for i, r in enumerate(residuals):
        out.append(store.add(bucket, float(i), [0.0] * 8, r)[0])
    return out


def test_gate_does_not_learn_from_a_garbage_shot():
    store = CalibrationStore()
    _feed(store, [0.2, -0.1, 0.1, 0.0, 0.15, -0.05, 0.1])
    before = store.calibration("45")[0][0]
    learned = store.add("45", 99.0, [0.0] * 8, 9.0)[0]  # e.g. something put on the scale
    assert not learned
    assert store.calibration("45")[0][0] == pytest.approx(before)


def test_three_in_a_row_same_direction_is_learned_as_a_real_change():
    store = CalibrationStore()
    _feed(store, [0.1, 0.0, -0.1, 0.05, 0.0, 0.1])
    results = _feed(store, [2.5, 2.5, 2.5])
    assert results[:2] == [False, False] and results[2] is True
    assert all(r["learned"] for r in store.buckets["45"][-3:])
    # recency-weighted, shrunk mean of 6 shots near 0 and 3 shots at 2.5 g
    w_new = sum(math.exp(-k / C.TAU) for k in range(3))
    w_old = sum(math.exp(-k / C.TAU) for k in range(3, 9))
    old_mean = sum([0.1, 0.0, -0.1, 0.05, 0.0, 0.1]) / 6
    expected = (2.5 * w_new + old_mean * w_old) / (w_new + w_old + C.LAM_OFFSET)
    assert store.calibration("45")[0][0] == pytest.approx(expected, abs=0.02)


def test_mixed_direction_rejections_stay_rejected():
    store = CalibrationStore()
    _feed(store, [0.1, 0.0, -0.1, 0.05, 0.0, 0.1])
    assert _feed(store, [6.0, -6.0, 6.0]) == [False, False, False]


def test_new_retraction_setting_uses_machine_wide_calibration_until_it_has_8_shots():
    store = CalibrationStore()
    _feed(store, [0.6] * 8, bucket="68")
    cal_new, source = store.calibration("45")
    assert "machine-wide" in source and cal_new[0] > 0.3
    _feed(store, [-0.4] * 7, bucket="45")
    assert "machine-wide" in store.calibration("45")[1]  # 7 shots: not trusted on their own yet
    _feed(store, [-0.4], bucket="45")
    cal_45, source = store.calibration("45")
    assert "retraction 45" in source and cal_45[0] < 0


def test_store_round_trip(tmp_path):
    path = str(tmp_path / "cal.json")
    store = CalibrationStore(path, "ll-test")
    _feed(store, [0.3, 0.2, 0.4])
    store.save()
    again = CalibrationStore(path, "ll-test")
    assert again.loaded
    assert again.calibration("45")[0] == pytest.approx(store.calibration("45")[0])


def test_calibration_learned_for_another_model_is_discarded(tmp_path):
    path = str(tmp_path / "cal.json")
    store = CalibrationStore(path, "ll-old")
    _feed(store, [0.3, 0.2, 0.4])
    store.save()
    fresh = CalibrationStore(path, "ll-new")
    assert not fresh.loaded and fresh.buckets == {}


# ---------------- 5 s label ----------------
def test_label_is_the_weight_five_seconds_after_the_stop():
    tr = PostDecisionTrace(100.0, 34.0)
    t = 100.0
    while not tr.add(t := t + 0.14, 34.0 + min(2.0, (t - 100.0) * 1.0)):
        pass
    assert tr.label() == pytest.approx(36.0, abs=1e-6)


def test_no_label_when_the_cup_is_lifted_early():
    tr = PostDecisionTrace(0.0, 34.0)
    tr.add(1.0, 35.5)
    tr.add(2.0, 36.0)
    assert tr.add(3.0, -300.0)  # cup lifted
    assert tr.label() is None


# ---------------- stored shot replay: exact parity with the training pipeline ----------------
def test_replay_reproduces_training_inputs_and_label():
    fx = load_fixture()
    got = [(x, y) for x, y in replay(fx["data"]) if y is not None]
    assert len(got) == 1
    x, y = got[0]
    exp = fx["expected"]
    assert y == pytest.approx(exp["label"], abs=1e-6)
    assert len(x) == len(exp["features"]) == 48
    for a, b in zip(x, exp["features"]):
        if b is None:
            assert a is None or math.isnan(a)
        else:
            assert a == pytest.approx(b, rel=1e-4, abs=1e-4)


# ---------------- live path: samples in, calibration message out ----------------
def _run_live(calibrator, linear_learning_prediction, linear_learning_control=None):
    """Feed the fixture shot as the live serial loop does (each Data line with its own Sensors line)."""
    rows = load_fixture()["data"]
    for n, r in enumerate(rows):
        shot, sens = r["shot"], (rows[n - 1] if n else r)["sensors"]
        sp = shot.get("setpoints") or {}
        act = sp.get("active")
        data = SimpleNamespace(
            status=r["status"],
            pressure=shot["pressure"],
            flow=shot["flow"],
            weight=shot["weight"],
            gravimetric_flow=shot["gravimetric_flow"],
            main_controller_kind=act,
            main_setpoint=sp.get(act) if act else 0.0,
        )
        sensors = SimpleNamespace(
            linear_learning_prediction=linear_learning_prediction,
            linear_learning_control=linear_learning_control,
            **{
                k: sens.get(k)
                for k in ("motor_position", "motor_speed", "motor_power", "motor_current")
            },
        )
        calibrator.on_sample(data, sensors, t=r["profile_ms"] / 1000.0)


def test_live_samples_produce_a_calibration_message(tmp_path):
    sent = []
    cal = LinearLearningCalibrator(str(tmp_path / "cal.json"), sent.append, lambda: 45.33)
    _run_live(cal, linear_learning_prediction="NaN")
    calib = [m for m in sent if m.startswith("linear_learning_calib,")]
    assert len(calib) == 1, "one calibration, sent once the shot has been learned"
    parts = calib[0].rstrip("\x03").split(",")
    assert parts[:2] == ["linear_learning_calib", cal.model.model_id]
    assert len(parts) == 3 + 9 and all(math.isfinite(float(v)) for v in parts[3:])
    # LinearLearning has not earned the stop: the stock prediction keeps it
    control = [m for m in sent if m.startswith("linear_learning_control,")]
    assert control and set(control) == {f"linear_learning_control,{cal.model.model_id},0\x03"}
    assert os.path.exists(tmp_path / "cal.json")


def test_nothing_is_sent_to_firmware_without_linear_learning(tmp_path):
    sent = []
    cal = LinearLearningCalibrator(str(tmp_path / "cal.json"), sent.append, lambda: 45.33)
    _run_live(cal, linear_learning_prediction=None)
    assert sent == []
    assert os.path.exists(tmp_path / "cal.json")  # still learned, ready for a firmware update


def test_bootstrap_from_stored_debug_shots(tmp_path):
    day = tmp_path / "debug" / "2026-10-04"
    day.mkdir(parents=True)
    with open(FIXTURE, "rb") as src, open(day / "191518.shot.json.zst", "wb") as dst:
        dst.write(src.read())
    store_path = str(tmp_path / "cal.json")
    sent = []
    cal = LinearLearningCalibrator(
        store_path, sent.append, lambda: 45.33, history_path=str(tmp_path / "debug")
    )
    assert cal.needs_bootstrap
    assert cal.bootstrap() == 1
    assert sent == []  # the ESP has not reported LinearLearning support yet
    again = LinearLearningCalibrator(store_path, sent.append, lambda: 45.33)
    assert not again.needs_bootstrap
    assert again.store.calibration("45")[0] == pytest.approx(cal.store.calibration("45")[0])


def test_shot_finishing_during_the_bootstrap_is_learned_after_the_stored_shots(tmp_path):
    day = tmp_path / "debug" / "2026-10-04"
    day.mkdir(parents=True)
    stored = dict(load_fixture(), time=1000.0)
    with open(day / "191518.shot.json.zst", "wb") as f:
        f.write(zstandard.ZstdCompressor().compress(json.dumps(stored).encode()))
    cal = LinearLearningCalibrator(
        str(tmp_path / "cal.json"),
        [].append,
        lambda: 45.33,
        history_path=str(tmp_path / "debug"),
    )
    cal.bootstrapping = True  # a live shot finishes while the bootstrap is still running
    _run_live(cal, linear_learning_prediction=None)
    assert len(cal.deferred) == 1 and cal.store.buckets == {}
    assert cal.bootstrap() == 1
    times = [r["t"] for r in cal.store.buckets["45"]]
    assert times[0] == 1000.0 and times[1] > times[0] and not cal.deferred


# ---------------- earned control on the live path ----------------
def _store_with_control(path, model_id, control):
    with open(path, "w") as f:
        json.dump(
            dict(version=1, model_id=model_id, buckets={}, pending={}, control=control), f
        )


def test_a_coffee_stopped_by_weight_is_scored_for_earned_control(tmp_path):
    path = str(tmp_path / "cal.json")
    cal = LinearLearningCalibrator(path, [].append, lambda: 45.33, target_weight=lambda: 36)
    _run_live(cal, linear_learning_prediction=35.9)
    label = load_fixture()["expected"]["label"]
    rec = cal.control.records[-1]
    assert rec["phase"] == "ghost" and rec["arm"] == "stock"
    assert rec["cup"] == pytest.approx(label - 36, abs=1e-3)
    assert rec["ll"] == pytest.approx(label - 35.9, abs=1e-3)
    with open(path) as f:
        assert json.load(f)["control"]["records"] == cal.control.records  # persisted


def test_a_coffee_not_stopped_by_weight_is_not_scored(tmp_path):
    for target in (45, None, 2000):  # far from the cup; unknown; "no final weight"
        cal = LinearLearningCalibrator(
            str(tmp_path / f"cal{target}.json"),
            [].append,
            lambda: 45.33,
            target_weight=lambda: target,
        )
        _run_live(cal, linear_learning_prediction=35.9)
        assert cal.control.records == []


def test_linear_learning_gets_the_stop_only_once_earned_and_allowed(tmp_path):
    model_id = FleetModel().model_id
    for allow, expected in ((True, "1"), (False, "0")):
        path = str(tmp_path / f"cal_{allow}.json")
        _store_with_control(path, model_id, dict(phase="live", records=[], live_count=0))
        sent = []
        cal = LinearLearningCalibrator(
            path, sent.append, lambda: 45.33, target_weight=lambda: 36, allow_control=allow
        )
        _run_live(cal, linear_learning_prediction=35.9, linear_learning_control=allow)
        assert sent[-1] == f"linear_learning_control,{model_id},{expected}\x03"
        assert cal.control.records[-1]["arm"] == ("ll" if allow else "stock")

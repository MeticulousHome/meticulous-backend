"""Main-board learner for LinearLearning, the ESP32's final-weight predictor.

Fed every ESP sample (Data + Sensors pair, correctly paired as they arrive). At each retraction decision it
captures the 48 inputs, reads the scale 5 s later, scores the fleet model, updates the per-retraction
calibration (calibration.py) and sends the 9 numbers to the ESP32:
    "linear_learning_calib,<model id>,<calibration id>,<9 numbers>\\x03"
It also scores each coffee stopped by weight for earned control (control.py) and tells the ESP32 who decides
the next coffee's final-weight stop:
    "linear_learning_control,<model id>,<0|1>\\x03"
Both are sent after every coffee, when the ESP (re)connects and when the retraction setting changes, but only to
firmware that runs LinearLearning (it reports linear_learning_prediction on the Sensors line).
On first start (or after a model change) it bootstraps the calibration from the machine's stored debug shots.
"""

import glob
import json
import logging
import math
import os
import threading
import time

from .calibration import CalibrationStore, bucket_key
from .control import WEIGHT_STOP_G, EarnedControl
from .labeler import PostDecisionTrace
from .model import FleetModel
from .stream import NON_SHOT, FeatureStream

try:  # the backend's logger when running inside meticulous-backend
    from log import MeticulousLogger

    logger = MeticulousLogger.getLogger(__name__)
except Exception:  # standalone (tests, offline replays)
    logger = logging.getLogger("linear_learning")

BOOTSTRAP_MAX_FILES = 200  # most recent stored shots used to bootstrap
BOOTSTRAP_PAUSE_S = 0.02  # between files, to keep the backend responsive
NO_WEIGHT_STOP_G = 1000.0  # profiles use 1000 / 2000 g as "no final weight"


def _setpoint(kind, value):
    return float(value) if kind not in (None, "none", "") and value is not None else 0.0


def _number(v):
    """a logged value as a float, or None (missing, "NaN", not a number)"""
    try:
        v = float(v)
    except (TypeError, ValueError):
        return None
    return v if math.isfinite(v) else None


def _target(final_weight):
    w = _number(final_weight)
    return w if w is not None and 0 < w < NO_WEIGHT_STOP_G else None


class _Pipeline:
    """stream + decision detection + 5 s labelling over one sample sequence"""

    def __init__(self):
        self.stream = FeatureStream()
        self.prev_status = None
        self.prev_info = None
        self.last_t = None
        self.pending = None

    def step(self, t, status, P, q, w, g, sp, pos, spd, pwr, cur, info=None):
        """returns (features, label, info at the decision) when a shot's label becomes available, else None"""
        done = None
        if self.pending is not None and self.pending[0].add(t, w):
            trace, x, inf = self.pending
            self.pending = None
            done = (x, trace.label(), inf)
        if (
            status == "retracting"
            and self.prev_status is not None
            and self.prev_status not in NON_SHOT
        ):
            x = self.stream.features()  # inputs at the decision (the previous sample)
            if x is not None:
                self.pending = (PostDecisionTrace(self.last_t, x[10]), x, self.prev_info)
                self.pending[0].add(t, w)
        self.stream.push(t, status, P, q, w, g, sp, pos, spd, pwr, cur)
        self.prev_status, self.last_t, self.prev_info = status, t, info
        return done


class LinearLearningCalibrator:
    def __init__(
        self,
        store_path,
        send,
        retraction_mm,
        history_path=None,
        model=None,
        target_weight=None,
        allow_control=True,
    ):
        self.model = model or FleetModel()
        self.store = CalibrationStore(store_path, self.model.model_id)
        self.send = send  # callable(str): raw message to the ESP
        self.retraction_mm = retraction_mm  # callable() -> current retraction setting (mm)
        self.history_path = history_path
        # callable() -> the final weight (g) of the profile being brewed, None if unknown
        self.target_weight = target_weight or (lambda: None)
        self.allow_control = (
            allow_control  # False: LinearLearning never gets the stop (ghost only)
        )
        self.control = EarnedControl(self.store.control)
        self.decides = False  # what the ESP was last told: LinearLearning has the stop
        self.live = _Pipeline()
        self.lock = threading.Lock()
        self.calib_id = int(time.time()) % 1000000
        self.last_bucket = None
        self.firmware_supported = False  # set once the ESP reports linear_learning_prediction
        self.needs_bootstrap = not self.store.loaded
        self.bootstrapping = False
        self.deferred = []  # live shots that finished while bootstrapping, learned after it

    # ---------------- live ----------------
    def on_sample(self, data, sensors, t=None):
        t = time.monotonic() if t is None else t
        try:
            if (
                not self.firmware_supported
                and getattr(sensors, "linear_learning_prediction", None) is not None
            ):
                self.firmware_supported = True
                self.last_bucket = None  # send the calibration now
            info = dict(
                ll=_number(getattr(sensors, "linear_learning_prediction", None)),
                arm="ll" if getattr(sensors, "linear_learning_control", None) else "stock",
            )
            got = self.live.step(
                t,
                data.status,
                data.pressure,
                data.flow,
                data.weight,
                data.gravimetric_flow,
                _setpoint(data.main_controller_kind, data.main_setpoint),
                sensors.motor_position,
                sensors.motor_speed,
                sensors.motor_power,
                sensors.motor_current,
                info=info,
            )
            if got is not None:
                x, label, inf = got
                self._learn(x, label, bucket_key(self.retraction_mm()), time.time(), inf)
            if bucket_key(self.retraction_mm()) != self.last_bucket:
                self.push()
        except Exception:
            logger.exception("LinearLearning calibration failed on a sample")

    def _learn(self, x, label, bucket, when, info=None):
        if label is None:
            logger.info("LinearLearning: shot not used (no stable reading 5 s after the stop)")
            return
        z = self.model.key_z(x)
        residual = label - (x[10] + self.model.fleet_drip(x))
        score = self._score(label, info)
        with self.lock:
            if self.bootstrapping:  # keep the records chronological: after the stored shots
                self.deferred.append((bucket, when, z, residual, score))
                logger.info("LinearLearning: shot will be learned once the bootstrap is done")
                return
            learned, reason = self.store.add(bucket, when, z, residual)
            change = self._record(score)
            self.store.save()
        logger.info(
            f"LinearLearning: shot at retraction {bucket} mm, final {label:.2f} g, "
            f"fleet residual {residual:+.2f} g: {reason}"
        )
        self._log_score(score, change)
        self.push()

    def _score(self, label, info):
        """(arm, cup - target, cup - LinearLearning at the stop) for a coffee stopped by weight, else None"""
        target = _target(self.target_weight())
        if info is None or target is None or abs(label - target) > WEIGHT_STOP_G:
            return None
        ll = info.get("ll")
        return (info.get("arm", "stock"), label - target, None if ll is None else label - ll)

    def _record(self, score):
        """add a scored coffee to the earned-control record (caller holds the lock)"""
        if score is None:
            return None
        change = self.control.add(*score)
        self.store.control = self.control.state()
        return change

    def _log_score(self, score, change):
        if score is None:
            return
        arm, cup, ll = score
        who = "LinearLearning" if arm == "ll" else "the stock prediction"
        ll_text = "" if ll is None else f", LinearLearning {ll:+.2f} g from the cup"
        logger.info(
            f"LinearLearning: {who} decided this coffee: cup {cup:+.2f} g from the target{ll_text} "
            f"(earned control: {self.control.phase})"
        )
        if change:
            logger.warning(f"LinearLearning: {change}")

    def push(self):
        """send the calibration for the current retraction setting, and who decides the next coffee, to the ESP
        (if it runs LinearLearning)"""
        bucket = bucket_key(self.retraction_mm())
        self.last_bucket = bucket
        if not self.firmware_supported:
            return
        with self.lock:
            any_learned = any(r["learned"] for b in self.store.buckets.values() for r in b)
            cal, source = self.store.calibration(bucket)
            decides = self.allow_control and self.control.linear_learning_decides_next()
            phase = self.control.phase
        if any_learned:  # nothing learned yet: the ESP keeps its compiled calibration
            self.calib_id = (self.calib_id + 1) % 1000000
            self.send(
                f"linear_learning_calib,{self.model.model_id},{self.calib_id},"
                + ",".join(f"{c:.5f}" for c in cal)
                + "\x03"
            )
            logger.info(
                f"LinearLearning: sent calibration {self.calib_id} ({source}): "
                + ", ".join(f"{c:+.3f}" for c in cal)
            )
        self.send(f"linear_learning_control,{self.model.model_id},{1 if decides else 0}\x03")
        if decides != self.decides:
            who = "LinearLearning" if decides else "the stock prediction"
            logger.info(f"LinearLearning: {who} decides the next final-weight stop ({phase})")
        self.decides = decides

    # ---------------- bootstrap from stored shots ----------------
    def bootstrap(self):
        """Learn the calibration from the machine's stored debug shots (first start, or after a model change).
        Returns the number of stored shots learned from. Earned control starts from new coffees only.
        """
        with self.lock:
            self.bootstrapping = True
        try:
            added, files = self._learn_history()
        finally:
            with self.lock:
                self.bootstrapping = False
                for bucket, when, z, residual, score in self.deferred:
                    self.store.add(bucket, when, z, residual)
                    self._record(score)
                self.deferred.clear()
                self.store.save()
        logger.info(
            f"LinearLearning: bootstrapped calibration from {added} stored shots ({files} files)"
        )
        self.push()
        return added

    def _learn_history(self):
        if not self.history_path or not os.path.isdir(self.history_path):
            return 0, 0
        import zstandard

        files = sorted(glob.glob(os.path.join(self.history_path, "*", "*.shot.json.zst")))[
            -BOOTSTRAP_MAX_FILES:
        ]
        added = 0
        for path in files:
            try:
                with open(path, "rb") as f:
                    js = json.loads(zstandard.ZstdDecompressor().stream_reader(f).read())
            except Exception:
                continue
            retraction = (js.get("machine") or {}).get("partial_retraction")
            for x, label in replay(js.get("data") or []):
                if label is None:
                    continue
                z = self.model.key_z(x)
                with self.lock:
                    self.store.add(
                        bucket_key(retraction),
                        js.get("time") or os.path.getmtime(path),
                        z,
                        label - (x[10] + self.model.fleet_drip(x)),
                    )
                added += 1
            time.sleep(BOOTSTRAP_PAUSE_S)
        return added, len(files)


def replay(rows):
    """Replay a stored shot (debug history 'data' rows): (inputs, label) per decision. The stored rows carry the
    Sensors line of the NEXT sample (the backend attaches each Sensors line to the previous row), so take
    sensors from the row before.
    """
    pipe, out = _Pipeline(), []
    for n, r in enumerate(rows):
        shot = r.get("shot") or {}
        sens = (rows[n - 1] if n > 0 else r).get("sensors") or {}
        sp = shot.get("setpoints") or {}
        act = sp.get("active")
        got = pipe.step(
            r.get("profile_ms", 0) / 1000.0,
            r.get("status"),
            shot.get("pressure"),
            shot.get("flow"),
            shot.get("weight"),
            shot.get("gravimetric_flow"),
            _setpoint(act, sp.get(act) if act else None),
            sens.get("motor_position"),
            sens.get("motor_speed"),
            sens.get("motor_power"),
            sens.get("motor_current"),
        )
        if got is not None:
            out.append(got[:2])
    if pipe.pending is not None:  # shot ended within 6 s of the stop: maybe still labelable
        trace, x, _ = pipe.pending
        out.append((x, trace.label()))
    return out

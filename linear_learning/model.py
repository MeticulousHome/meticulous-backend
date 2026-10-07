"""Fleet part of LinearLearning (the same constants the ESP32 firmware is built with, see model_id)."""

import json
import math
import os

PARAMS_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fleet_params.json")
N_CAL = 9  # offset + 8 sensitivities


def _clip(x, lo, hi):
    return lo if x < lo else hi if x > hi else x


class FleetModel:
    def __init__(self, path=PARAMS_PATH):
        with open(path) as f:
            p = json.load(f)
        self.model_id = p["model_id"]  # must equal MODEL_ID compiled into the ESP32 firmware
        self.med, self.mu, self.sd, self.coef = p["med"], p["mu"], p["sd"], p["coef"]
        self.b0 = p["b0"]
        self.key_idx, self.k_mu, self.k_sd = p["key_idx"], p["k_mu"], p["k_sd"]
        self.default_cal = list(p["cal"])  # calibration compiled into the firmware
        self.clip_fleet, self.clip_cal = p.get("clip_fleet_sd", 4.0), p.get(
            "clip_calib_sd", 3.0
        )

    def _filled(self, x):
        return [self.med[i] if (v is None or math.isnan(v)) else v for i, v in enumerate(x)]

    def fleet_drip(self, x):
        """drip predicted by the fleet part alone (grams to add to the current weight)"""
        x = self._filled(x)
        return self.b0 + sum(
            c * _clip((v - m) / s, -self.clip_fleet, self.clip_fleet)
            for c, v, m, s in zip(self.coef, x, self.mu, self.sd)
        )

    def key_z(self, x):
        """the 8 standardized inputs the per-machine calibration uses"""
        x = self._filled(x)
        return [
            _clip((x[i] - m) / s, -self.clip_cal, self.clip_cal)
            for i, m, s in zip(self.key_idx, self.k_mu, self.k_sd)
        ]

    @staticmethod
    def calibration_term(cal, z):
        return cal[0] + sum(c * v for c, v in zip(cal[1:], z))

    def predict(self, x, cal):
        """predicted final weight = weight now (x[10]) + fleet drip + calibration"""
        return (
            self._filled(x)[10] + self.fleet_drip(x) + self.calibration_term(cal, self.key_z(x))
        )

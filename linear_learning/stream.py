"""Streaming inputs of LinearLearning (the final-weight predictor), one ESP sample (Data + Sensors pair) at a time.

Pure-Python port of the reference used to train the model. It must stay in exact parity with the ESP32's
lib/LinearLearning (EspressoFirmware), which computes the same 48 inputs from the same printed values.
"""

import math
from collections import deque

NON_SHOT = {
    "heating",
    "purge",
    "retracting",
    "closing valve",
    "remove cup",
    "home",
    "starting...",
    "idle",
    "click to start",
    "click to purge",
    "preheating",
}
ML_PER_MM = 2.0
RING = 64  # raw samples kept (>= 3.1 s at the ~8 Hz UART rate)
CV_MAX = 200  # closing-valve samples kept
MIN_SHOT_S = 8.0  # predictions are only available >= 8 s into an extraction
N_FEAT = 48
_CH = ("P", "q", "w", "g", "sp", "pos", "spd", "pwr", "cur")
_IX = {n: i for i, n in enumerate(_CH)}


def _nz(x):
    try:
        x = float(x)
    except (TypeError, ValueError):
        return 0.0
    return 0.0 if math.isnan(x) else x


def _slope(n, sx, sy, sxx, sxy):
    den = n * sxx - sx * sx
    return (n * sxy - sx * sy) / den if n >= 2 and den != 0 else float("nan")


class _Onset:
    """pressure build after the 'closing valve' stage starts"""

    def __init__(self, t0, pos0):
        self.t0, self.pos0 = t0, pos0
        self.found = self.too_late = False
        self.v_on = self.t_on = self.s_on = 0.0
        self.v = [
            0,
            0.0,
            0.0,
            0.0,
            0.0,
        ]  # n, sx, sy, sxx, sxy for P vs volume (6 ml after onset)
        self.v_min, self.v_max, self.p_last6, self.sp6, self.n6, self.closed6 = (
            math.inf,
            -math.inf,
            float("nan"),
            0.0,
            0,
            False,
        )
        self.tt = [0, 0.0, 0.0, 0.0, 0.0]  # P vs time (1.5 s after onset)
        self.t_closed = False
        self.k1, self.v1, self.t1 = False, float("nan"), float("nan")

    def add(self, t, P, pos, spd):
        tt, vv = t - self.t0, (pos - self.pos0) * ML_PER_MM
        is_onset = False
        if not self.found:
            if P < 0.3:
                return
            if tt > 60:
                self.too_late = True
            self.found, is_onset = True, True
            self.v_on, self.t_on, self.s_on = vv, tt, spd
        if self.too_late:
            return
        if not self.closed6:
            if not is_onset and vv > self.v_on + 6:
                self.closed6 = True
            elif self.v_on <= vv <= self.v_on + 6:
                a = self.v
                a[0] += 1
                a[1] += vv
                a[2] += P
                a[3] += vv * vv
                a[4] += vv * P
                self.v_min, self.v_max = min(self.v_min, vv), max(self.v_max, vv)
                self.p_last6, self.sp6, self.n6 = P, self.sp6 + P, self.n6 + 1
        if not self.t_closed:
            if self.t_on <= tt <= self.t_on + 1.5:
                a = self.tt
                a[0] += 1
                a[1] += tt
                a[2] += P
                a[3] += tt * tt
                a[4] += tt * P
            elif tt > self.t_on + 1.5:
                self.t_closed = True
        if not self.k1 and P >= 1.0:
            self.k1, self.v1, self.t1 = True, vv - self.v_on, tt - self.t_on

    def features(self):
        nan = float("nan")
        if not self.found or self.too_late:
            return [nan] * 9
        dpdv = pafter = air = nan
        if self.n6 >= 3 and (self.v_max - self.v_min) > 1:
            dpdv, pafter = _slope(*self.v), self.p_last6
            if dpdv > 1e-3:
                air = (1 + self.sp6 / self.n6) / dpdv
        dpdt = _slope(*self.tt) if self.tt[0] >= 3 else nan
        return [self.v_on, self.t_on, self.s_on, dpdv, dpdt, air, self.v1, self.t1, pafter]


class _Shot:
    def __init__(self, t, P, pos, w, onset):
        self.tj, self.posj, self.wj, self.onset = t, pos, w, onset
        self.k03 = self.k1 = self.k4 = self.kd = None
        self.pmax = -math.inf

    def add(self, t, P, pos, w):
        v = (pos - self.posj) * ML_PER_MM
        if self.k03 is None and P >= 0.3:
            self.k03 = (t, P, v)
        k1_earlier = self.k1 is not None
        if self.k1 is None and P >= 1.0:
            self.k1 = (t, P, v)
        if self.k4 is None and P >= 4.0:
            self.k4 = (t, P, v, k1_earlier)
        if self.kd is None and w - self.wj >= 1.0:
            self.kd = (t, P, v)
        self.pmax = max(self.pmax, P)


class FeatureStream:
    """push() every sample in order; features() returns the 48 inputs at the latest sample (or None)."""

    def __init__(self):
        self.ring = deque(maxlen=RING)
        self.prev = None
        self.cv = deque(maxlen=CV_MAX)
        self.shot = None

    def push(
        self,
        t,
        status,
        pressure,
        flow,
        weight,
        grav_flow,
        setpoint,
        position,
        speed,
        power,
        current,
    ):
        P, q, w, g, sp, pos, spd, pwr, cur = map(
            _nz, (pressure, flow, weight, grav_flow, setpoint, position, speed, power, current)
        )
        t, status, prev = float(t), str(status), self.prev
        if status == "closing valve":
            if prev != "closing valve":
                self.cv.clear()
            self.cv.append((t, P, pos, spd))
        if status not in NON_SHOT:
            if prev is None or prev in NON_SHOT:  # an extraction starts at this sample
                if prev == "closing valve" and self.cv:
                    t0, _, p0, _ = self.cv[0]
                    onset = _Onset(t0, p0)
                    for c in self.cv:
                        onset.add(*c)
                else:
                    onset = _Onset(t, pos)
                self.shot = _Shot(t, P, pos, w, onset)
            self.shot.onset.add(t, P, pos, spd)
            self.shot.add(t, P, pos, w)
        else:
            self.shot = None
        if status in NON_SHOT and status != "closing valve" and prev == "closing valve":
            self.cv.clear()
        self.ring.append((t, P, q, w, g, sp, pos, spd, pwr, cur))
        self.prev = status

    def _grid(self, ch, r):
        """channel value at t_now - 0.1 r, linear interpolation clamped at the ends (numpy.interp semantics)"""
        rows, c = self.ring, 1 + _IX[ch]
        x = rows[-1][0] - 0.1 * r
        if x <= rows[0][0]:
            return rows[0][c]
        if x >= rows[-1][0]:
            return rows[-1][c]
        a = len(rows) - 2
        while a > 0 and rows[a][0] > x:
            a -= 1
        ta, tb = rows[a][0], rows[a + 1][0]
        return rows[a][c] + (rows[a + 1][c] - rows[a][c]) / (tb - ta) * (x - ta)

    def features(self):
        s = self.shot
        if s is None or len(self.ring) < 2:
            return None
        now = self.ring[-1]
        t_now = now[0]
        if t_now - s.tj < MIN_SHOT_S or t_now - self.ring[0][0] < 3.05:
            return None
        G = {
            n: [self._grid(n, r) for r in range(30)]
            for n in ("P", "q", "w", "g", "sp", "spd", "pwr", "cur")
        }

        def m(n, a, b=0):  # mean of grid points b..a-1 (0 = newest, 0.1 s apart)
            return sum(G[n][b:a]) / (a - b)

        W = G["w"]
        w0 = now[1 + _IX["w"]]
        late = [
            m("g", 10),
            m("g", 30),
            m("q", 5),
            m("q", 20),
            m("P", 5),
            m("P", 20),
            W[0] - W[10],
            m("sp", 5),
            now[1 + _IX["pos"]],
            m("spd", 5),
            w0,
            t_now - s.tj,
        ]
        inst = [
            G["g"][0],
            G["q"][0],
            G["P"][0],
            (W[0] - W[5]) / 0.5,
            (W[0] - W[20]) / 2.0,
            m("g", 10) - m("g", 30, 20),
            m("q", 10) - m("q", 30, 20),
            m("P", 10) - m("P", 30, 20),
            m("P", 5) / max(m("q", 5), 0.1),
            m("g", 10) / max(m("q", 10), 0.1),
            m("pwr", 5),
            m("cur", 5),
        ]
        nan = float("nan")
        P_now, pos_now = now[1 + _IX["P"]], now[1 + _IX["pos"]]
        v_now = (pos_now - s.posj) * ML_PER_MM
        v14 = t14 = dpdv = air0 = airr = nan
        if s.k1 is not None and s.k4 is not None and s.k4[3]:
            dv = s.k4[2] - s.k1[2]
            v14, t14 = dv, s.k4[0] - s.k1[0]
            dpdv = (s.k4[1] - s.k1[1]) / max(dv, 0.1)
            air0 = dv / (1 / (1 + s.k1[1]) - 1 / (1 + s.k4[1]))
            airr = air0 * P_now / (1 + P_now)
        recent = [r[1 + _IX["P"]] for r in self.ring if r[0] >= t_now - 3 and r[0] >= s.tj]
        kd = s.kd
        phys = [
            s.k03[2] if s.k03 else nan,
            v14,
            t14,
            air0,
            airr,
            v_now,
            v_now - w0,
            (kd[0] - s.tj) if kd else nan,
            kd[2] if kd else nan,
            kd[1] if kd else nan,
            P_now,
            s.pmax,
            sum(recent) / len(recent) if recent else nan,
            dpdv,
            pos_now,
        ]
        return late + inst + s.onset.features() + phys

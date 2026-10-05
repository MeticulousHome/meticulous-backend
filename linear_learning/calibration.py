"""Per-machine calibration of LinearLearning (the final-weight predictor), kept per retraction setting.

After every shot the calibration (offset + 8 sensitivities) is refit on earlier shots at the same retraction
setting, recency weighted (weight halves about every 7 shots), shrunk towards zero, residuals capped at 3 g.
Protection against bad shots (ruined puck, something put on the scale):
  - gate: a shot whose error is far from the recent norm (> max(1.5 g, 4 x robust spread of the last 20
    learned shots)) is recorded but not learned from;
  - but 3 rejected shots in a row that are off in the same direction are a real change: they are learned.
While a retraction setting has fewer than MIN_BUCKET learned shots, the machine-wide calibration is used.
Validated by replaying real shot histories with injected garbage and with genuine changes.
"""

import json
import math
import os
import tempfile

N_CAL = 9
TAU = 10.0  # recency: weight = exp(-age / TAU)  (half-life ~7 shots)
LAM_OFFSET, LAM_SLOPE = 1.0, 5.0
CLIP_G = 3.0
GATE_MIN_G, GATE_SPREAD_K, GATE_WARMUP, GATE_WINDOW = 1.5, 4.0, 5, 20
SAME_DIRECTION_RUN = 3
MIN_BUCKET = 3
MAX_RECORDS = 100  # per bucket (older shots weigh < 0.01%)


def _solve(A, b):
    """Gaussian elimination with partial pivoting (small dense systems)."""
    n = len(b)
    M = [row[:] + [b[i]] for i, row in enumerate(A)]
    for col in range(n):
        piv = max(range(col, n), key=lambda r: abs(M[r][col]))
        if abs(M[piv][col]) < 1e-12:
            return [0.0] * n
        M[col], M[piv] = M[piv], M[col]
        for r in range(col + 1, n):
            f = M[r][col] / M[col][col]
            if f:
                for c in range(col, n + 1):
                    M[r][c] -= f * M[col][c]
    x = [0.0] * n
    for r in range(n - 1, -1, -1):
        x[r] = (M[r][n] - sum(M[r][c] * x[c] for c in range(r + 1, n))) / M[r][r]
    return x


def fit(records):
    """records: chronological list of (z[8], residual); returns the 9 calibration numbers."""
    if not records:
        return [0.0] * N_CAL
    k = len(records)
    AtA = [[0.0] * N_CAL for _ in range(N_CAL)]
    Atb = [0.0] * N_CAL
    for i, (z, r) in enumerate(records):
        w = math.exp(-(k - 1 - i) / TAU)
        a = [1.0] + list(z)
        rr = max(-CLIP_G, min(CLIP_G, r))
        for p in range(N_CAL):
            Atb[p] += w * a[p] * rr
            for q in range(N_CAL):
                AtA[p][q] += w * a[p] * a[q]
    AtA[0][0] += LAM_OFFSET
    for p in range(1, N_CAL):
        AtA[p][p] += LAM_SLOPE
    return _solve(AtA, Atb)


def _median(v):
    s = sorted(v)
    n = len(s)
    return 0.0 if n == 0 else (s[n // 2] if n % 2 else 0.5 * (s[n // 2 - 1] + s[n // 2]))


def bucket_key(retraction_mm):
    try:
        v = float(retraction_mm)
    except (TypeError, ValueError):
        return "unknown"
    return "unknown" if math.isnan(v) else str(int(round(v)))


class CalibrationStore:
    """Shot records per retraction bucket; each record: t, z (8 inputs), r (fleet residual),
    e (error vs the calibration that was in use), learned (bool)."""

    def __init__(self, path=None, model_id=None):
        self.path = path
        self.model_id = model_id
        self.buckets = {}
        self.pending = (
            {}
        )  # bucket -> consecutive rejected records (indices into the bucket list)
        self.loaded = False
        if path and os.path.exists(path):
            with open(path) as f:
                data = json.load(f)
            if (
                data.get("model_id") == model_id
            ):  # residuals are only meaningful for the same fleet model
                self.buckets = data.get("buckets", {})
                self.pending = data.get("pending", {})
                self.loaded = True

    # ---------- calibration ----------
    def _learned(self, bucket):
        return [(r["z"], r["r"]) for r in self.buckets.get(bucket, []) if r["learned"]]

    def _machine_learned(self):
        allr = [r for b in self.buckets.values() for r in b if r["learned"]]
        allr.sort(key=lambda r: r["t"])
        return [(r["z"], r["r"]) for r in allr]

    def calibration(self, bucket):
        own = self._learned(bucket)
        if len(own) >= MIN_BUCKET:
            return fit(own), f"retraction {bucket} mm ({len(own)} shots)"
        machine = self._machine_learned()
        return (
            fit(machine),
            f"machine-wide fallback ({len(machine)} shots; {len(own)} at retraction {bucket} mm)",
        )

    # ---------- learning ----------
    def add(self, bucket, t, z, residual):
        """Record one finished shot and decide whether to learn from it. Returns (learned, reason)."""
        cal, _ = self.calibration(bucket)
        e = residual - (cal[0] + sum(c * v for c, v in zip(cal[1:], z)))
        recs = self.buckets.setdefault(bucket, [])
        recent = [r["e"] for r in recs if r["learned"]][-GATE_WINDOW:]
        learn, reason = True, "learned"
        if len(recent) >= GATE_WARMUP:
            med = _median(recent)
            spread = 1.4826 * _median([abs(v - med) for v in recent])
            if abs(e - med) > max(GATE_MIN_G, GATE_SPREAD_K * spread):
                learn, reason = (
                    False,
                    f"not learned: error {e:+.2f} g is outside the recent norm ({med:+.2f} g)",
                )
        recs.append(dict(t=t, z=list(z), r=residual, e=e, learned=learn))
        pend = self.pending.setdefault(bucket, [])
        if learn:
            pend.clear()
        else:
            pend.append(len(recs) - 1)
            last = pend[-SAME_DIRECTION_RUN:]
            if (
                len(last) == SAME_DIRECTION_RUN
                and len({math.copysign(1, recs[i]["e"]) for i in last}) == 1
            ):
                for i in last:
                    recs[i]["learned"] = True
                pend.clear()
                learn, reason = (
                    True,
                    f"learned: {SAME_DIRECTION_RUN} shots in a row off in the same direction (real change)",
                )
        if len(recs) > MAX_RECORDS:
            drop = len(recs) - MAX_RECORDS
            del recs[:drop]
            self.pending[bucket] = [i - drop for i in self.pending.get(bucket, []) if i >= drop]
        return learn, reason

    def save(self):
        if not self.path:
            return
        os.makedirs(os.path.dirname(self.path) or ".", exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=os.path.dirname(self.path) or ".", suffix=".tmp")
        with os.fdopen(fd, "w") as f:
            json.dump(
                dict(
                    version=1,
                    model_id=self.model_id,
                    buckets=self.buckets,
                    pending=self.pending,
                ),
                f,
            )
        os.replace(tmp, self.path)

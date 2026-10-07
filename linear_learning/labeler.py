"""Final weight of a shot = scale weight 5 s after the retraction decision (median of 4.5 / 5.0 / 5.5 s,
linear interpolation), the label the predictor was trained on. No label if the cup was lifted (weight
fell > 1 g below its running max) before 6 s, or if the trace is implausible."""

import math

LABEL_T = 5.0
CUP_LIFT_G = 1.0
DONE_AFTER_S = LABEL_T + 1.0


class PostDecisionTrace:
    def __init__(self, t0, w0):
        self.t0, self.w0 = t0, w0
        self.samples = [(0.0, w0)]
        self.runmax = w0
        self.lifted_at = None

    def add(self, t, w):
        """feed scale samples after the decision; returns True once enough has been seen"""
        dt = t - self.t0
        if w is None or (isinstance(w, float) and math.isnan(w)):
            w = 0.0
        if self.lifted_at is None:
            if w < self.runmax - CUP_LIFT_G:
                self.lifted_at = dt
            self.runmax = max(self.runmax, w)
        self.samples.append((dt, w))
        return dt >= DONE_AFTER_S or self.lifted_at is not None

    def _at(self, x):
        s = self.samples
        if x <= s[0][0]:
            return s[0][1]
        for (ta, wa), (tb, wb) in zip(s, s[1:]):
            if ta <= x <= tb:
                return wa if tb == ta else wa + (wb - wa) * (x - ta) / (tb - ta)
        return s[-1][1]

    def label(self):
        """final weight, or None when this shot cannot be labelled"""
        if self.lifted_at is not None and self.lifted_at - 0.5 < LABEL_T + 0.5:
            return None
        if self.samples[-1][0] < LABEL_T + 0.5:
            return None
        vals = sorted(self._at(LABEL_T + o) for o in (-0.5, 0.0, 0.5))
        y = vals[1]
        if not (5 < y < 500) or self.w0 < 3 or not (-2 <= y - self.w0 <= 30):
            return None
        return y

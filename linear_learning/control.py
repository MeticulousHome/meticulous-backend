"""Earned control: LinearLearning decides a machine's final-weight stop only after it has beaten the stock
prediction on that machine, judged on what matters: how far the cup ended from the target (scale weight 5 s
after the stop vs the profile's final weight).

  ghost  (start)  The stock prediction decides every coffee. LinearLearning is scored on the same coffees (its
                  prediction at the stop vs the cup). That score flatters it: the predictor that decides also
                  pays for firing a little past the target (~0.12 g at 10 Hz) on an upward blip (~0.09 g),
                  measured on a test machine. So a trial starts only if, over the last 20 coffees,
                  LinearLearning beat the stock by more than MARGIN_G (one-sided paired t-test, p < 0.05).
  trial           Coffees alternate (whichever has fewer scored coffees decides the next one). After at least
                  15 each, LinearLearning takes over if its cups ended closer to the target (Welch t-test,
                  p < 0.025). The trial ends early, back to ghost, if after 10 each LinearLearning's cups are
                  clearly further from the target (p < 0.05); and after 30 each without a win.
  live            LinearLearning decides, except every 5th coffee, which the stock decides to keep a fresh
                  reference. LinearLearning hands the stop back (ghost) as soon as the stock's cups are closer
                  to the target over the recent coffees (Welch t-test, p < 0.1).
Back in ghost, a new trial needs 20 new coffees. Only coffees stopped by weight count: the profile has a
final weight and the cup ended within WEIGHT_STOP_G of it (a stop for time, pressure or by hand ends further).
"""

import math

GHOST, TRIAL, LIVE = "ghost", "trial", "live"
MARGIN_G = 0.15
SCREEN_N, SCREEN_P = 20, 0.05
TRIAL_MIN, TRIAL_MAX, TRIAL_P = 15, 30, 0.025
TRIAL_HARM_MIN, TRIAL_HARM_P = 10, 0.05
LIVE_REFERENCE_EVERY, LIVE_WINDOW, LIVE_MIN, FALLBACK_P = 5, 15, 5, 0.10
WEIGHT_STOP_G = 3.0
MAX_RECORDS = 400


# ---------------- Student's t (no scipy on the machine) ----------------
def _betacf(a, b, x):
    """continued fraction of the incomplete beta function (modified Lentz)"""
    tiny = 1e-300
    c, d = 1.0, 1.0 - (a + b) * x / (a + 1.0)
    d = 1.0 / (d if abs(d) > tiny else tiny)
    h = d
    for m in range(1, 300):
        for aa in (
            m * (b - m) * x / ((a + 2 * m - 1) * (a + 2 * m)),
            -(a + m) * (a + b + m) * x / ((a + 2 * m) * (a + 2 * m + 1)),
        ):
            d = 1.0 + aa * d
            d = 1.0 / (d if abs(d) > tiny else tiny)
            c = 1.0 + aa / c
            c = c if abs(c) > tiny else tiny
            h *= d * c
        if abs(d * c - 1.0) < 1e-14:
            break
    return h


def _betai(a, b, x):
    """regularized incomplete beta function I_x(a, b)"""
    if x <= 0.0:
        return 0.0
    if x >= 1.0:
        return 1.0
    ln = (
        math.lgamma(a + b)
        - math.lgamma(a)
        - math.lgamma(b)
        + a * math.log(x)
        + b * math.log1p(-x)
    )
    if x < (a + 1.0) / (a + b + 2.0):
        return math.exp(ln) * _betacf(a, b, x) / a
    return 1.0 - math.exp(ln) * _betacf(b, a, 1.0 - x) / b


def t_sf(t, df):
    """P(T > t) for Student's t distribution with df degrees of freedom"""
    tail = 0.5 * _betai(df / 2.0, 0.5, df / (df + t * t))
    return tail if t > 0 else 1.0 - tail


def _mean(v):
    return sum(v) / len(v)


def _var(v):
    m = _mean(v)
    return sum((x - m) ** 2 for x in v) / (len(v) - 1)


def p_mean_above_zero(d):
    """one-sided p-value that the mean of d is above zero (t-test)"""
    if len(d) < 3:
        return 1.0
    m, v = _mean(d), _var(d)
    if v <= 0.0:
        return 0.0 if m > 0 else 1.0
    return t_sf(m / math.sqrt(v / len(d)), len(d) - 1)


def p_first_mean_larger(a, b):
    """one-sided p-value that mean(a) > mean(b) (Welch's t-test)"""
    if len(a) < 2 or len(b) < 2:
        return 1.0
    sa, sb = _var(a) / len(a), _var(b) / len(b)
    if sa + sb <= 0.0:
        return 0.0 if _mean(a) > _mean(b) else 1.0
    df = (sa + sb) ** 2 / (sa**2 / (len(a) - 1) + sb**2 / (len(b) - 1))
    return t_sf((_mean(a) - _mean(b)) / math.sqrt(sa + sb), df)


# ---------------- per-machine state ----------------
class EarnedControl:
    """Who decides the final-weight stop on this machine. State is a plain dict, persisted by the caller."""

    def __init__(self, state=None):
        s = state or {}
        self.phase = s.get("phase", GHOST)
        # chronological, one per coffee stopped by weight: phase, arm ("ll" | "stock"), cup, ll (g)
        self.records = list(s.get("records", []))
        self.live_count = int(s.get("live_count", 0))

    def state(self):
        return dict(phase=self.phase, records=self.records, live_count=self.live_count)

    def _current(self):
        """records of the current phase: the trailing run recorded in it"""
        n = len(self.records)
        while n > 0 and self.records[n - 1]["phase"] == self.phase:
            n -= 1
        return self.records[n:]

    def linear_learning_decides_next(self):
        if self.phase == TRIAL:
            cur = self._current()
            n_ll = sum(1 for r in cur if r["arm"] == "ll")
            return n_ll <= len(cur) - n_ll
        if self.phase == LIVE:
            return (self.live_count + 1) % LIVE_REFERENCE_EVERY != 0
        return False

    def add(self, arm, cup_error, ll_error=None):
        """Record a coffee stopped by weight. arm: "ll" if LinearLearning had the stop, else "stock";
        cup_error: cup - target (g); ll_error: cup - LinearLearning's prediction at the stop (None if it had
        none). Returns a sentence describing the phase change it caused, or None."""
        self.records.append(
            dict(
                phase=self.phase,
                arm=arm,
                cup=round(cup_error, 3),
                ll=None if ll_error is None else round(ll_error, 3),
            )
        )
        del self.records[:-MAX_RECORDS]
        if self.phase == GHOST:
            return self._screen()
        if self.phase == TRIAL:
            return self._trial()
        self.live_count += 1
        return self._live()

    def _screen(self):
        rs = [r for r in self._current() if r["arm"] == "stock" and r["ll"] is not None][
            -SCREEN_N:
        ]
        if len(rs) < SCREEN_N:
            return None
        d = [abs(r["cup"]) - abs(r["ll"]) - MARGIN_G for r in rs]
        p = p_mean_above_zero(d)
        if _mean(d) > 0 and p < SCREEN_P:
            self.phase = TRIAL
            return (
                f"trial starts: over the last {SCREEN_N} coffees LinearLearning was "
                f"{_mean([abs(r['ll']) for r in rs]):.2f} g from the cup, the stock's cups "
                f"{_mean([abs(r['cup']) for r in rs]):.2f} g from the target (p = {p:.3f})"
            )
        return None

    def _trial(self):
        cur = self._current()
        ll = [abs(r["cup"]) for r in cur if r["arm"] == "ll"]
        st = [abs(r["cup"]) for r in cur if r["arm"] == "stock"]
        n = min(len(ll), len(st))
        if n < TRIAL_HARM_MIN:
            return None

        def summary(p):
            return (
                f"LinearLearning's cups ended {_mean(ll):.2f} g from the target, the stock's "
                f"{_mean(st):.2f} g ({len(ll)} and {len(st)} coffees, p = {p:.3f})"
            )

        harm = p_first_mean_larger(ll, st)
        if _mean(ll) > _mean(st) and harm < TRIAL_HARM_P:
            self.phase = GHOST
            return f"trial stopped early, the stock keeps the stop: {summary(harm)}"
        if n < TRIAL_MIN:
            return None
        p = p_first_mean_larger(st, ll)
        if _mean(ll) < _mean(st) and p < TRIAL_P:
            self.phase, self.live_count = LIVE, 0
            return f"LinearLearning takes over the stop: in the trial {summary(p)}"
        if n >= TRIAL_MAX:
            self.phase = GHOST
            return f"trial over, the stock keeps the stop: {summary(p)}"
        return None

    def _live(self):
        cur = self._current()
        refs = [i for i, r in enumerate(cur) if r["arm"] == "stock"][-LIVE_WINDOW:]
        if len(refs) < LIVE_MIN:
            return None
        span = cur[refs[0] :]
        st = [abs(r["cup"]) for r in span if r["arm"] == "stock"]
        ll = [abs(r["cup"]) for r in span if r["arm"] == "ll"]
        p = p_first_mean_larger(ll, st)
        if _mean(ll) > _mean(st) and p < FALLBACK_P:
            self.phase = GHOST
            return (
                f"LinearLearning hands the stop back: the stock's cups ended {_mean(st):.2f} g from the "
                f"target, LinearLearning's {_mean(ll):.2f} g ({len(st)} and {len(ll)} coffees, p = {p:.3f})"
            )
        return None

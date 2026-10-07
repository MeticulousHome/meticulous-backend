"""Tests for earned control: LinearLearning gets a machine's final-weight stop only after it beat the stock
prediction there, judged on how far the cup ended from the target."""

import pytest

from linear_learning import control as K
from linear_learning.control import GHOST, LIVE, TRIAL, EarnedControl


def cycle(values, n):
    """n values cycling through a list (deterministic spread)"""
    return [values[i % len(values)] for i in range(n)]


def test_t_tests_match_reference_values():
    # reference values from scipy.stats (not a dependency on the machine)
    assert K.t_sf(2.0, 10) == pytest.approx(0.03669401738537018, abs=1e-12)
    assert K.t_sf(-1.2, 7.5) == pytest.approx(0.866665538603389, abs=1e-12)
    assert K.p_first_mean_larger(
        [0.9, 1.1, 0.7, 1.3, 0.8], [0.4, 0.6, 0.5, 0.3, 0.7, 0.2]
    ) == pytest.approx(0.002700755745874818, abs=1e-12)
    assert K.p_mean_above_zero([0.1, 0.3, -0.05, 0.2, 0.15]) == pytest.approx(
        0.03642752980512784, abs=1e-12
    )


# ---------------- ghost: the stock prediction decides ----------------
def test_stock_keeps_the_stop_while_linear_learning_is_not_clearly_better():
    c = EarnedControl()
    # LinearLearning 0.05 g closer: less than what deciding costs it
    cups, lls = cycle([0.5, -0.3, 0.4, -0.5], 40), cycle([0.45, -0.25, 0.35, -0.45], 40)
    for cup, ll in zip(cups, lls):
        assert c.add("stock", cup, ll) is None
        assert not c.linear_learning_decides_next()
    assert c.phase == GHOST


def test_a_trial_starts_when_linear_learning_would_clearly_have_done_better():
    c = EarnedControl()
    cups, lls = cycle([0.8, -0.7, 0.9, -0.6], 20), cycle([0.2, -0.1, 0.25, -0.15], 20)
    msgs = [c.add("stock", cup, ll) for cup, ll in zip(cups, lls)]
    assert all(m is None for m in msgs[:-1])  # needs 20 coffees
    assert c.phase == TRIAL and msgs[-1].startswith("trial starts")


def test_coffees_without_a_linear_learning_prediction_do_not_count_for_the_screen():
    c = EarnedControl()
    for _ in range(30):
        c.add("stock", 0.8, None)
    assert c.phase == GHOST


# ---------------- trial: alternate, then decide ----------------
def run_trial(c, ll_cups, stock_cups):
    msgs, li, si = [], iter(ll_cups), iter(stock_cups)
    while c.phase == TRIAL:
        if c.linear_learning_decides_next():
            msgs.append(c.add("ll", next(li)))
        else:
            msgs.append(c.add("stock", next(si)))
    return [m for m in msgs if m]


def test_trial_alternates_who_decides():
    c = EarnedControl(dict(phase=TRIAL))
    order = []
    for _ in range(6):
        arm = "ll" if c.linear_learning_decides_next() else "stock"
        order.append(arm)
        c.add(arm, 0.3)
    assert order == ["ll", "stock"] * 3


def test_linear_learning_takes_over_when_its_cups_end_closer_to_the_target():
    c = EarnedControl(dict(phase=TRIAL))
    msgs = run_trial(c, cycle([0.2, -0.15, 0.25, -0.1], 99), cycle([0.7, -0.5, 0.8, -0.6], 99))
    assert (
        c.phase == LIVE
        and msgs == [msgs[0]]
        and msgs[0].startswith("LinearLearning takes over")
    )
    trial = [r for r in c.records if r["phase"] == TRIAL]
    assert sum(r["arm"] == "ll" for r in trial) >= K.TRIAL_MIN
    assert sum(r["arm"] == "stock" for r in trial) >= K.TRIAL_MIN


def test_a_draw_leaves_the_stop_with_the_stock():
    c = EarnedControl(dict(phase=TRIAL))
    same = cycle([0.4, -0.3, 0.5, -0.2], 99)
    msgs = run_trial(c, same, same)
    assert c.phase == GHOST and msgs[0].startswith("trial over")
    assert len([r for r in c.records if r["phase"] == TRIAL]) == 2 * K.TRIAL_MAX


def test_a_trial_stops_early_when_linear_learning_is_clearly_worse():
    c = EarnedControl(dict(phase=TRIAL))
    msgs = run_trial(c, cycle([0.9, -0.8, 1.0, -0.7], 99), cycle([0.2, -0.15, 0.25, -0.1], 99))
    assert c.phase == GHOST and msgs[0].startswith("trial stopped early")
    trial = [r for r in c.records if r["phase"] == TRIAL]
    assert len(trial) == 2 * K.TRIAL_HARM_MIN  # 10 each, not 30


def test_after_a_lost_trial_a_new_one_needs_20_new_coffees():
    c = EarnedControl(dict(phase=TRIAL))
    same = cycle([0.4, -0.3, 0.5, -0.2], 99)
    run_trial(c, same, same)
    for i in range(19):
        assert c.add("stock", 0.8 if i % 2 else -0.7, 0.1) is None
    assert c.add("stock", 0.8, -0.1).startswith("trial starts")


# ---------------- live: LinearLearning decides, the stock keeps a reference ----------------
def test_live_keeps_every_5th_coffee_for_the_stock():
    c = EarnedControl(dict(phase=LIVE))
    arms = []
    for _ in range(10):
        arm = "ll" if c.linear_learning_decides_next() else "stock"
        arms.append(arm)
        c.add(arm, 0.2 if arm == "ll" else 0.6)
    assert arms == ["ll"] * 4 + ["stock"] + ["ll"] * 4 + ["stock"]
    assert c.phase == LIVE


def test_linear_learning_hands_the_stop_back_when_the_stock_does_better():
    c = EarnedControl(dict(phase=LIVE))
    ll, st = iter(cycle([0.8, -0.9, 0.7, -0.85], 99)), iter(cycle([0.2, -0.15, 0.25, -0.1], 99))
    n, msg = 0, None
    while c.phase == LIVE:
        n += 1
        if c.linear_learning_decides_next():
            msg = c.add("ll", next(ll))
        else:
            msg = c.add("stock", next(st))
    assert n == K.LIVE_REFERENCE_EVERY * K.LIVE_MIN  # as soon as there are 5 stock references
    assert c.phase == GHOST and msg.startswith("LinearLearning hands the stop back")
    assert not c.linear_learning_decides_next()


def test_state_survives_a_restart():
    c = EarnedControl(dict(phase=TRIAL))
    c.add("ll", 0.2, 0.1)
    again = EarnedControl(c.state())
    assert again.phase == TRIAL and again.records == c.records
    assert not again.linear_learning_decides_next()  # the stock's turn

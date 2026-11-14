"""Backend benchmark on the real firmware (fika-sil), the same for any backend.

Knob latency: fika-sil turns the encoder one detent; the time until a
Socket.IO client receives the backend's `button` event. The firmware's share
(debounce, UART line) is the same for every backend, so differences between
two runs are the backends'.

CPU and memory: the backend process's CPU time and RSS while idle and over
one espresso shot of the default profile.

Writes artifacts/benchmark.json.
"""

import json
import os
import statistics
import time

import pytest

from conftest import ARTIFACTS, BACKEND_DIR, wall_seconds

TURNS = int(os.environ.get("BENCH_TURNS", "120"))
CLK = os.sysconf("SC_CLK_TCK")


def backend_pid(backend) -> int:
    """The process doing the backend's work: the Rust binary the harness
    starts, or the harness itself, which runs the Python backend in-process."""
    harness = backend.proc.pid
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            stat = open(f"/proc/{entry}/stat").read()
        except OSError:
            continue
        comm = stat[stat.index("(") + 1 : stat.rindex(")")]
        ppid = int(stat[stat.rindex(")") + 2 :].split()[1])
        if ppid == harness and comm.startswith("meticulous-bac"):
            return int(entry)
    return harness


def cpu_seconds(pid: int) -> float:
    stat = open(f"/proc/{pid}/stat").read()
    fields = stat[stat.rindex(")") + 2 :].split()
    return (int(fields[11]) + int(fields[12])) / CLK


def rss_kib(pid: int) -> int:
    for line in open(f"/proc/{pid}/status"):
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return 0


def percentile(values, p):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round(p / 100 * (len(ordered) - 1))))]


@pytest.mark.journey("espresso-complete", tier="simulated")
def test_backend_benchmark(backend, firmware, dial):
    pid = backend_pid(backend)
    dial.wait_for(lambda s: s["name"] == "idle", wall_seconds(120), "the idle status")
    time.sleep(5)
    result = {"pid_is_harness": pid == backend.proc.pid}

    # Idle: 30 s of status streaming to one client.
    c0, t0 = cpu_seconds(pid), time.monotonic()
    time.sleep(30)
    result["idle_cpu_percent"] = 100 * (cpu_seconds(pid) - c0) / (time.monotonic() - t0)
    result["idle_rss_kib"] = rss_kib(pid)

    # Knob latency.
    latencies = []
    for _ in range(TURNS):
        before = len(dial.events)
        sent = time.monotonic()
        firmware.call("rotate", detents=1, ms_per_detent=50)
        deadline = sent + 2.0
        got = None
        while time.monotonic() < deadline and got is None:
            for at, event, data in dial.events[before:]:
                if event == "button":
                    got = at
                    break
            time.sleep(0.0005)
        if got is not None:
            latencies.append((got - sent) * 1000)
        time.sleep(0.25)
    result["knob_turns"] = TURNS
    result["knob_events_received"] = len(latencies)
    if latencies:
        result["knob_latency_ms"] = {
            "p50": round(percentile(latencies, 50), 2),
            "p90": round(percentile(latencies, 90), 2),
            "p99": round(percentile(latencies, 99), 2),
            "max": round(max(latencies), 2),
            "mean": round(statistics.mean(latencies), 2),
        }

    # One shot of the default profile, answering the prompts like the Dial.
    profile = json.loads((BACKEND_DIR / "simple_profile.json").read_text())
    firmware.event("new_puck")
    firmware.event("place_cup")
    c0, t0 = cpu_seconds(pid), time.monotonic()
    peak = rss_kib(pid)
    status_code, _ = backend.post("/api/v1/profile/load", profile)
    assert status_code == 200
    prompts = {"click to start", "click to purge", "remove cup"}
    answered, seen = set(), []
    stage_names = {stage["name"] for stage in profile["stages"]}
    deadline = time.monotonic() + wall_seconds(20 * 60)
    statuses_before = len(dial.statuses())
    while time.monotonic() < deadline:
        name = (dial.last_status() or {}).get("name")
        if name and (not seen or seen[-1] != name):
            seen.append(name)
        if name == "Pour water and click to continue" and name not in answered:
            firmware.call("event", name="pour_water", ml=60)
            time.sleep(wall_seconds(1))
            dial.emit("action", "continue")
            answered.add(name)
        if name in prompts and name not in answered:
            time.sleep(wall_seconds(1))
            dial.emit("action", "continue")
            answered.add(name)
        if name == "idle" and stage_names <= set(seen):
            break
        peak = max(peak, rss_kib(pid))
        time.sleep(0.05)
    elapsed = time.monotonic() - t0
    result["shot_seconds"] = round(elapsed, 1)
    result["shot_cpu_percent"] = 100 * (cpu_seconds(pid) - c0) / elapsed
    result["shot_peak_rss_kib"] = peak
    result["shot_statuses"] = len(dial.statuses()) - statuses_before
    result["shot_status_rate_hz"] = round(result["shot_statuses"] / elapsed, 2)
    result["status_sequence"] = seen

    (ARTIFACTS / "benchmark.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
    assert latencies, "no button event reached the client"

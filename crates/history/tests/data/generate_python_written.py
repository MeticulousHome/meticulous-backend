"""A history written by the Python backend, and what the Python reads back.

Builds python_written/: history.sqlite, shot_files/ (the shots/ directory,
with "_" for ":" in file names)
and pour_over_files/ (pour-over/), written through ShotDataBase, ShotManager's
zstd CLI call, PourOverHistoryManager and the bug report functions; and
python_written/expected.json with the answers of the Python read paths. The
Rust tests open a copy and must answer the same. Run with TZ=UTC (the shot
times are local wall clock times):

    TZ=UTC uv run python crates/history/tests/data/generate_python_written.py
"""

import asyncio
import json
import shutil
import sqlite3
import subprocess
from typing import Any, cast

from pyenv import DATA_DIR, Env, compact

import pour_over_history
from api.history import ZstdHistoryHandler
from config import CONFIG_SYSTEM, MACHINE_SERIAL_NUMBER, MeticulousConfig
from pour_over_history import PourOverHistoryManager, PourOverSession
from shot_database import SearchParams, ShotDataBase
from shot_manager import ShotManager

OUT = DATA_DIR / "python_written"


def profile(profile_id, name, stages, temperature=93.0, **extra):
    return {
        "id": profile_id,
        "author": "Meticulous",
        "author_id": "author-0001",
        "display": {"accentColor": "#FF7A00", "image": "/api/v1/profile/image/x.png"},
        "final_weight": 36.0,
        "last_changed": 1760000000.0,
        "name": name,
        "temperature": temperature,
        "stages": [
            {"key": f"{profile_id}-{i}", "name": stage, "type": "pressure"}
            for i, stage in enumerate(stages)
        ],
        "variables": [
            {"key": "pressure_1", "name": "Pressure", "type": "pressure", "value": 9}
        ],
        "previous_authors": [],
        **extra,
    }


CLASSIC = profile("p-classic", "Classic", ["Preinfusion", "Extraction"])
CLASSIC_V2 = profile("p-classic-v2", "Classic", ["Preinfusion", "Extraction"], 94.5)
BLOOM = profile("p-bloom", "Bloom café", ["Bloom", "Pour"], 92, author=None)


def point(time_ms, pressure, weight, flow=1.5):
    return {
        "shot": {
            "pressure": pressure,
            "flow": 2.0,
            "weight": weight,
            "gravimetric_flow": flow,
            "setpoints": {"active": "pressure", "pressure": 9.0},
        },
        "time": time_ms,
        "profile_time": time_ms - 200,
        "status": "brewing",
    }


def write_shot(env, shot):
    """ShotManager.stop()'s write_current_shot: zstd -10 of json.dumps(ensure_ascii=False)."""
    folder, file_path = ShotManager._timestampToFilePaths(shot["time"])
    (env.history / "shots" / folder).mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["zstd", "-10", "-f", "-q", "-o", str(env.history / "shots" / file_path)],
        input=json.dumps(shot, ensure_ascii=False).encode("utf-8"),
        check=True,
    )
    return str(file_path)


class FakeHandler:
    def __init__(self):
        self.headers = {}
        self.body = b""

    def set_header(self, key, value):
        self.headers[key] = value

    def write(self, chunk):
        self.body += chunk

    def finish(self):
        pass


def served(path):
    handler = FakeHandler()
    asyncio.run(
        ZstdHistoryHandler.serve_decompressed_file(cast(ZstdHistoryHandler, handler), str(path))
    )
    return {"content_type": handler.headers["Content-Type"], "body": handler.body.decode()}


def session(session_id, started_at, mode="free_pour"):
    return {
        "schemaVersion": 4,
        "id": session_id,
        "brewType": "pour_over",
        "mode": mode,
        "name": "Free Pour" if mode == "free_pour" else "Three Pour",
        "source": "dial",
        "startedAt": started_at,
        "completedAt": started_at.replace(":00.000Z", ":45.120Z"),
        "recipe": {
            "profileId": None if mode == "free_pour" else "profile-1",
            "profileName": "Free Pour",
            "doseG": 15,
            "temperatureC": 92,
            "targetWaterG": None,
            "targetDurationMs": None,
            "pourTargets": [],
        },
        "measurements": {
            "serverBaselineG": -0.4,
            "setupG": 149.2,
            "waterPouredG": 225.3,
            "beverageG": 196.8,
            "retainedG": 28.5,
            "durationMs": 135_000,
            "status": "measured",
        },
        "pours": [],
        "samples": [{"t": 0, "w": 0, "f": 0.8, "p": 1}],
        "completion": "brewer_removed",
        "sync": {"status": "pending", "attempts": 0},
        "appVersion": "café 1.0",
    }


def main():
    import api.bug_report as bug_report

    env = Env("python-written")
    assert env.boot() is None
    pour_over_history.POUR_OVER_PATH = env.history / "pour-over"
    pour_over_history.MIN_POUR_OVER_FREE_BYTES = 0
    MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = "MACHINE-001"

    shots = [
        {
            "time": 1760000000.0,
            "profile": CLASSIC,
            "data": [point(1000, 9.0, 0.0), point(1100, 8.5, 0.4)],
            "ptb": 1500,
        },
        {"time": 1760000060.5, "profile": BLOOM, "data": [point(1000, 2.0, 1.0)], "ptb": 0},
        {
            "time": 1760000120.25,
            "profile": CLASSIC_V2,
            "data": [point(1000, 9.1, float("inf"))],
            "ptb": 0,
        },
        {
            "time": 1760000180.123456,
            "profile": CLASSIC,
            "data": [point(1000, float("nan"), 2.0)],
            "ptb": 0,
        },
        {"time": 1760000240, "profile": BLOOM, "data": [], "ptb": 0},
    ]
    entries = []
    for index, shot in enumerate(shots):
        shot_data = {
            "time": shot["time"],
            "profile_name": shot["profile"]["name"],
            "data": shot["data"],
            "id": f"00000000-0000-4000-8000-00000000000{index}",
            "push_to_brew_time": shot["ptb"],
            "profile": shot["profile"],
        }
        shot_data["file"] = write_shot(env, shot_data)
        shot_data["db_key"] = ShotDataBase.insert_history(dict(shot_data))
        entries.append(shot_data)
    # The last shot's file is gone: search_history leaves it out.
    (env.history / "shots" / entries[4]["file"]).unlink()

    assert ShotDataBase.rate_shot(entries[0]["id"], "like")
    assert ShotDataBase.rate_shot(entries[1]["id"], "dislike")
    assert ShotDataBase.rate_shot(entries[2]["id"], None)
    ShotDataBase.link_debug_file(entries[0]["db_key"], "2025-10-09/08:53:20.shot.json.zst")

    bug_report._insert_report(
        {
            "localID": "018f0a2b-1234-7abc-8def-0123456789ab",
            "dateAndTime": 1760000300,
            "issueTime": 1760000000,
            "description": "Grinder jammed",
            "multimedia": 2,
            "machineID": "MACHINE-001",
            "attachments": {
                "debugFiles": {"user": ["a.zst"], "automatic": ["b.zst", "a.zst"]},
                "machineInfo": True,
                "machineLogs": False,
                "machineStatus": True,
            },
            "ticket": None,
        }
    )
    bug_report._update_report_db(
        "018f0a2b-1234-7abc-8def-0123456789ab", {"ticketNumber": 4242, "baseEventID": "base"}
    )
    bug_report._insert_queued_report(
        {
            "localID": "018f0a2b-1234-7abc-8def-0123456789ac",
            "ticket": 77,
            "issueTime": None,
            "description": "Steam wand",
            "name": "Ada",
            "email": "ada@example.com",
        },
        1760000400,
    )
    bug_report._insert_queued_report(
        {
            "localID": "018f0a2b-1234-7abc-8def-0123456789ad",
            "ticket": 78,
            "issueTime": 1760000100,
            "description": None,
            "name": None,
            "email": None,
        },
        1760000400,
    )

    saved = [
        PourOverHistoryManager.save(
            PourOverSession.model_validate(
                session("11111111-2222-4333-8444-555555555555", "2026-08-16T08:00:00.000Z")
            )
        ),
        PourOverHistoryManager.save(
            PourOverSession.model_validate(
                session(
                    "33333333-4444-4555-8666-777777777777",
                    "2026-08-16T09:30:00.000Z",
                    "profile",
                )
            )
        ),
    ]

    searches: list[dict[str, Any]] = [
        {},
        {"dump_data": False},
        {"sort": "asc", "max_results": 2},
        {"max_results": 0},
        {"query": "bloom"},
        {"query": "Preinfusion", "dump_data": False},
        {"ids": [entries[0]["id"], 2]},
        {"ids": ["p-classic-v2"]},
        {"ids": ["3"]},
        {"order_by": ["profile"], "sort": "asc", "dump_data": False},
        {"start_date": 1760000060.5, "dump_data": False},
        {"end_date": 1760000060.5, "dump_data": False},
        {"start_date": 0, "dump_data": False},
    ]
    report_row = bug_report._get_report_row("018f0a2b-1234-7abc-8def-0123456789ab")
    expected = {
        "entries": [{k: v for k, v in e.items() if k != "data"} for e in entries],
        "search": [
            {"params": params, "result": ShotDataBase.search_history(SearchParams(**params))}
            for params in searches
        ],
        "statistics": ShotDataBase.statistics(),
        "autocomplete": {
            prefix: ShotDataBase.autocomplete_profile_name(prefix)
            for prefix in ["", "bloom", "Pre", "nothing"]
        },
        "ratings": {e["id"]: ShotDataBase.get_shot_rating(e["id"]) for e in entries},
        "upload_index": [
            ShotDataBase.list_history_files(None, 200),
            ShotDataBase.list_history_files(entries[0]["file"], 1),
        ],
        "served": {e["file"]: served(env.history / "shots" / e["file"]) for e in entries[:4]},
        "pour_over": {
            "saved": [[history, created] for history, created in saved],
            "search": PourOverHistoryManager.search(),
            "search_asc_profile": PourOverHistoryManager.search(
                descending=False, mode="profile"
            ),
            "search_after": PourOverHistoryManager.search(after=saved[0][0]["file"]),
            "latest": PourOverHistoryManager.latest(),
        },
        "bug_reports": {
            "page": bug_report._list_report_page(0, 10),
            "page_small": bug_report._list_report_page(1, 1),
            "queued": bug_report._list_report_page(
                0, 10, bug_report._parse_fiql("status==queued")[0]
            ),
            "dispatch": [
                bug_report._dispatch_payload(r) for r in bug_report._queued_report_rows()
            ],
            "report_info": bug_report._row_to_report_info(report_row),
        },
    }
    env.close()

    connection = sqlite3.connect(env.db_file)
    connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    connection.close()
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir()
    shutil.copy(env.db_file, OUT / "history.sqlite")
    # ":" cannot appear in file names on every checkout: shot_files/ stores
    # HH_MM_SS names and the tests restore the colons.
    for path in (env.history / "shots").rglob("*.zst"):
        relative = path.relative_to(env.history / "shots")
        target = OUT / "shot_files" / str(relative).replace(":", "_")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(path, target)
    shutil.copytree(env.history / "pour-over", OUT / "pour_over_files")
    (OUT / "expected.json").write_text(compact(expected) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()

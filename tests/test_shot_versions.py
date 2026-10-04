import asyncio
import io
import json
from pathlib import Path
import sys
import types

import pytest
import zstandard as zstd
from tornado.httpclient import AsyncHTTPClient
from tornado.httpserver import HTTPServer
from tornado.netutil import bind_sockets
from tornado.web import Application

try:
    import pyprctl  # noqa: F401
except Exception:
    sys.modules["pyprctl"] = types.SimpleNamespace(set_name=lambda _name: None)

import ota
import shot_database as database
import shot_manager
from esp_serial.data import ESPInfo, ShotData
from machine import Machine
from ota import UpdateManager
from shot_manager import ShotManager


@pytest.fixture
def version_sources(tmp_path, monkeypatch):
    files = {
        "BUILD_DATE_FILE": "Thu, 01 Oct 2026 12:34:56 +0000",
        "BUILD_CHANNEL_FILE": "beta",
        "BUILD_VERSION_FILE": "2026M1443-beta",
        "REPO_INFO_FILE": (
            "## backend ##\nRepository: backend\nURL: PRIVATE_URL\nBranch: nightly\n"
            f"Commit: {'a' * 40}\nLast commit details:\n"
            "aaaaaaa - PRIVATE_SUBJECT (yesterday) <PRIVATE_AUTHOR>\n"
            "Modified files:\nPRIVATE_FILE\n"
        ),
    }
    for name, content in files.items():
        path = tmp_path / name
        path.write_text(content)
        monkeypatch.setattr(ota, name, str(path))
    for name in ("ROOTFS_BUILD_DATE", "CHANNEL", "VERSION", "REPO_INFO"):
        monkeypatch.setattr(UpdateManager, name, None)
    monkeypatch.setattr(
        Machine,
        "esp_info",
        ESPInfo(firmwareV="0.2.24-391-gb4c20ef", serialNumber="PRIVATE_SERIAL"),
    )
    for name in ("_current_shot", "_last_shot", "db_history_id"):
        monkeypatch.setattr(ShotManager, name, None)
    return {
        "software_version": "2026-10-01 12:34:56",
        "image_build_channel": "beta",
        "image_version": "2026M1443-beta",
        "repository_info": {"backend": {"branch": "nightly", "commit": "a" * 40}},
        "firmware_version": "0.2.24-391-gb4c20ef",
    }


def test_normal_shot_captures_version_sources_without_debug_details(version_sources):
    ShotManager.start(push_to_brew_time=654)
    current = ShotManager.getCurrentShot()
    assert current["machine"] == version_sources
    assert current["push_to_brew_time"] == 654
    assert "PRIVATE_" not in json.dumps(current)


def test_version_snapshot_survives_source_changes(version_sources, monkeypatch):
    ShotManager.start()
    Machine.esp_info.firmwareV = "new-firmware"
    UpdateManager.REPO_INFO["backend"]["commit"] = "b" * 40
    UpdateManager.REPO_INFO["backend"]["branch"] = "new-branch"
    monkeypatch.setattr(UpdateManager, "VERSION", "new-image")
    assert ShotManager.getCurrentShot()["machine"] == version_sources


def test_missing_versions_stay_unknown_even_if_info_arrives_later(version_sources):
    for name in (
        "BUILD_DATE_FILE",
        "BUILD_CHANNEL_FILE",
        "BUILD_VERSION_FILE",
        "REPO_INFO_FILE",
    ):
        Path(getattr(ota, name)).unlink()
    Machine.esp_info = None
    ShotManager.start()
    expected = {
        "software_version": None,
        "image_build_channel": None,
        "image_version": None,
        "repository_info": {},
        "firmware_version": None,
    }
    Machine.esp_info = ESPInfo(firmwareV="later-firmware")
    UpdateManager.VERSION = "later-image"
    assert ShotManager.getCurrentShot()["machine"] == expected


@pytest.mark.parametrize("unavailable", ["invalid-date", "unreadable-file"])
def test_bad_build_timestamp_does_not_prevent_shot(version_sources, unavailable):
    path = Path(ota.BUILD_DATE_FILE)
    if unavailable == "invalid-date":
        path.write_text("invalid-date")
    else:
        path.unlink()
        path.mkdir()
    ShotManager.start()
    assert ShotManager.getCurrentShot()["machine"] == {
        **version_sources,
        "software_version": None,
    }


@pytest.mark.parametrize(
    "details, revision", [("abc1234 - Subject <Author>", "abc1234"), ("unknown", None)]
)
def test_legacy_summary_retains_only_known_revision(version_sources, details, revision):
    Path(ota.REPO_INFO_FILE).write_text(
        f"## backend ##\nBranch: nightly\nCommit: HEAD\nLast commit details:\n{details}\n"
    )
    ShotManager.start()
    assert ShotManager.getCurrentShot()["machine"]["repository_info"] == {
        "backend": {"branch": "nightly", "commit": revision}
    }


def test_empty_firmware_is_explicitly_unknown(version_sources):
    Machine.esp_info.firmwareV = ""
    ShotManager.start()
    assert ShotManager.getCurrentShot()["machine"]["firmware_version"] is None


def test_debug_shot_keeps_its_existing_version_fields(version_sources, monkeypatch):
    from hostname import HostnameManager
    from shot_debug_manager import DebugShot
    from wifi import WifiManager

    monkeypatch.setattr(HostnameManager, "generateDeviceName", lambda: "test-device")
    monkeypatch.setattr(
        WifiManager, "getCurrentConfig", lambda: types.SimpleNamespace(hostname="test-host")
    )
    debug = DebugShot().to_json()
    assert debug["machine"]["software_version"] == version_sources["software_version"]
    assert debug["machine"]["image_version"] == version_sources["image_version"]
    assert debug["machine"]["firmware_version"] == version_sources["firmware_version"]
    assert debug["machine"]["repository_info"]["backend"]["commit"] == (
        "aaaaaaa - PRIVATE_SUBJECT (yesterday) <PRIVATE_AUTHOR>"
    )
    assert "push_to_brew_time" not in debug


async def download_shot(root, path):
    import sentry_sdk
    from api.history import ZstdHistoryHandler

    app = Application(
        [(r"/api/v1/history/files/(.*)", ZstdHistoryHandler, {"path": str(root)})]
    )
    sockets = bind_sockets(0, "127.0.0.1")
    port = sockets[0].getsockname()[1]
    server = HTTPServer(app)
    server.add_sockets(sockets)
    client = AsyncHTTPClient(force_instance=True)
    try:
        # Give the SDK's Tornado wrapper an isolated client without a transport.
        with sentry_sdk.isolation_scope() as scope:
            sdk_client = sentry_sdk.Client(dsn=None)
            scope.set_client(sdk_client)
            try:
                return await client.fetch(
                    f"http://127.0.0.1:{port}/api/v1/history/files/{path}"
                )
            finally:
                sdk_client.close()
    finally:
        client.close()
        server.stop()
        await server.close_all_connections()


def test_finalization_and_download_preserve_snapshot_and_old_files(
    version_sources, tmp_path, monkeypatch
):
    from named_thread import NamedThread

    root = tmp_path / "shots"
    monkeypatch.setattr(shot_manager, "SHOT_PATH", root)
    monkeypatch.setattr(database, "SHOT_PATH", root)
    monkeypatch.setattr(database, "HISTORY_PATH", str(tmp_path))
    monkeypatch.setattr(database, "DATABASE_URL", f"sqlite:///{tmp_path / 'history.sqlite'}")
    for name in ("engine", "session", "stage_fts_table", "profile_fts_table"):
        monkeypatch.setattr(database.ShotDataBase, name, None)
    ShotManager.init()
    database.metadata.create_all(database.ShotDataBase.engine)
    threads = []

    def tracked_thread(*args, **kwargs):
        thread = NamedThread(*args, **kwargs)
        threads.append(thread)
        return thread

    monkeypatch.setattr(shot_manager, "NamedThread", tracked_thread)
    try:
        ShotManager.start()
        profile = {
            "id": "test-profile",
            "name": "Test espresso",
            "author": "Test",
            "author_id": "test",
            "display": {},
            "final_weight": 36,
            "temperature": 93,
            "stages": [],
        }
        ShotManager._current_shot.profile = profile
        ShotManager._current_shot.profile_name = profile["name"]
        ShotManager.handleShotData(ShotData(pressure=6, flow=2, weight=36, time=100))
        relative_path = ShotManager.getCurrentShot()["file"]
        UpdateManager.VERSION = "updated-after-shot"
        Machine.esp_info = ESPInfo(firmwareV="updated-after-shot")
        ShotManager.stop()
        assert ShotManager._current_shot is None
        assert len(threads) == 1
        threads[0].join(timeout=10)
        assert not threads[0].is_alive()
        assert ShotManager.db_history_id is not None
        response = asyncio.run(download_shot(root, relative_path))
        saved = json.loads(response.body)
        assert saved["machine"] == version_sources
        assert saved["data"][0]["shot"]["pressure"] == 6
        # The compressed attachment and the decompressed endpoint carry the same snapshot.
        compressed = asyncio.run(download_shot(root, relative_path + "?compressed=1"))
        with zstd.ZstdDecompressor().stream_reader(io.BytesIO(compressed.body)) as reader:
            assert json.loads(reader.read()) == saved
        without_suffix = asyncio.run(download_shot(root, relative_path.removesuffix(".zst")))
        assert json.loads(without_suffix.body) == saved

        old = {"id": "old-shot", "time": 1, "profile_name": "Old", "data": []}
        (root / "old.shot.json.zst").write_bytes(
            zstd.ZstdCompressor().compress(json.dumps(old).encode())
        )
        old_response = asyncio.run(download_shot(root, "old.shot.json.zst"))
        assert json.loads(old_response.body) == old
    finally:
        for thread in threads:
            thread.join(timeout=10)
        database.ShotDataBase.engine.dispose()

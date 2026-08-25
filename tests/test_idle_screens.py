import base64
import hashlib
import io
import json
import stat
import tempfile
import threading
import zipfile
from pathlib import Path

import pytest
from tornado.web import Application
from tornado.testing import AsyncHTTPTestCase

import api.idle_screens as idle_screens
from api.idle_screens import IdleScreenError


PNG_1X1 = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII="
)


@pytest.fixture(autouse=True)
def idle_screen_root(tmp_path, monkeypatch):
    root = tmp_path.joinpath("idle-screens")
    monkeypatch.setattr(idle_screens, "IDLE_SCREENS_ROOT", root)
    idle_screens._locks.clear()
    return root


def _screen(package_id="custom:test", name="Test Screen", layers=None):
    return {
        "documentType": "meticulous-idle-screen",
        "schemaVersion": 2,
        "runtimeApi": 1,
        "id": package_id,
        "name": name,
        "viewport": {
            "width": 480,
            "height": 480,
            "shape": "circle",
            "background": "#000000",
            "overflow": "hidden",
        },
        "runtime": {
            "brightness": {"onEnter": 1, "onExit": 1},
            "burnInProtection": {
                "enabled": False,
                "mode": "none",
                "durationMs": 60000,
                "distance": 0,
            },
        },
        "tokens": {"colors": {}, "fonts": {}, "numbers": {}},
        "dataSources": [],
        "layers": layers or [],
    }


def _asset(asset_id, path, kind, mime_type, data, sha=None, size=None):
    return {
        "id": asset_id,
        "path": path,
        "kind": kind,
        "mimeType": mime_type,
        "size": len(data) if size is None else size,
        "sha256": hashlib.sha256(data).hexdigest() if sha is None else sha,
    }


def _manifest(package_id="custom:test", name="Test Screen", version="1.0.0", assets=None):
    return {
        "documentType": "meticulous-idle-manifest",
        "packageFormat": 1,
        "id": package_id,
        "name": name,
        "version": version,
        "runtimeApi": 1,
        "screen": "screen.json",
        "preview": "preview.png",
        "assets": assets or [],
    }


def _package(
    package_id="custom:test",
    name="Test Screen",
    version="1.0.0",
    assets=None,
    asset_files=None,
    layers=None,
    extra_entries=None,
    preview_data=PNG_1X1,
    manifest_overrides=None,
    screen_overrides=None,
):
    manifest = _manifest(package_id, name, version, assets)
    screen = _screen(package_id, name, layers)
    if manifest_overrides:
        manifest.update(manifest_overrides)
    if screen_overrides:
        screen.update(screen_overrides)

    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("manifest.json", json.dumps(manifest))
        archive.writestr("screen.json", json.dumps(screen))
        archive.writestr("preview.png", preview_data)
        for path, data in (asset_files or {}).items():
            archive.writestr(path, data)
        for entry in extra_entries or []:
            if len(entry) == 2:
                archive.writestr(entry[0], entry[1])
            else:
                info = zipfile.ZipInfo(entry[0])
                info.external_attr = entry[2]
                archive.writestr(info, entry[1])
    return out.getvalue()


def _assert_idle_error(package_bytes, code):
    with pytest.raises(IdleScreenError) as exc:
        idle_screens.validate_package(package_bytes)
    assert exc.value.code == code


def _mark_first_zip_entry_encrypted(package_bytes):
    data = bytearray(package_bytes)
    for signature, offset in [(b"PK\x03\x04", 6), (b"PK\x01\x02", 8)]:
        index = data.index(signature)
        flags = int.from_bytes(data[index + offset : index + offset + 2], "little")
        data[index + offset : index + offset + 2] = (flags | 0x1).to_bytes(2, "little")
    return bytes(data)


def test_vendored_schema_matches_authoritative_hash_and_bytes():
    schema_bytes = idle_screens.SCHEMA_PATH.read_bytes()
    assert hashlib.sha256(schema_bytes).hexdigest() == idle_screens.SCHEMA_SHA256

    authoritative = Path("/home/kerans/Documents/Projects/repos/internalTools")
    authoritative = authoritative.joinpath("idleScreenMaker", "idle-screen.schema.json")
    if authoritative.exists():
        assert schema_bytes == authoritative.read_bytes()


def test_list_first_install_replace_and_rollback_do_not_change_settings():
    first, replaced = idle_screens.install_package(
        _package(package_id="custom:clock", name="Clock", version="1.0.0")
    )
    assert replaced is False
    assert first["id"] == "custom:clock"
    assert first["rollbackAvailable"] is False
    assert idle_screens.list_installed_screens() == [first]

    second, replaced = idle_screens.install_package(
        _package(package_id="custom:clock", name="Clock Two", version="2.0.0")
    )
    assert replaced is True
    assert second["version"] == "2.0.0"
    assert second["rollbackAvailable"] is True

    rolled_back = idle_screens.rollback_package("custom:clock")
    assert rolled_back["version"] == "1.0.0"
    assert rolled_back["rollbackAvailable"] is True
    assert not any(
        path.name == "settings" for path in idle_screens.IDLE_SCREENS_ROOT.rglob("*")
    )


def test_invalid_directory_is_listed_with_safe_metadata(idle_screen_root):
    idle_screen_root.joinpath("custom__broken").mkdir(parents=True)
    screens = idle_screens.list_installed_screens()

    assert screens == [
        {
            "id": "custom__broken",
            "name": "custom__broken",
            "version": "",
            "packageHash": "",
            "installedAt": screens[0]["installedAt"],
            "rollbackAvailable": False,
            "valid": False,
        }
    ]


def test_reserved_id_is_rejected():
    _assert_idle_error(_package(package_id="default"), "reserved_id")


def test_structured_error_for_malformed_zip():
    with pytest.raises(IdleScreenError) as exc:
        idle_screens.validate_package(b"not a zip")
    assert exc.value.code == "malformed_zip"
    assert exc.value.details


def test_rejects_encrypted_entries():
    _assert_idle_error(_mark_first_zip_entry_encrypted(_package()), "encrypted_archive")


@pytest.mark.parametrize(
    "entry_name,code",
    [
        ("../manifest.json", "invalid_path"),
        ("/manifest.json", "invalid_path"),
        ("folder/manifest.json", "invalid_layout"),
    ],
)
def test_rejects_traversal_absolute_and_enclosing_root(entry_name, code):
    _assert_idle_error(_package(extra_entries=[(entry_name, b"bad")]), code)


def test_rejects_links():
    mode = (stat.S_IFLNK | 0o777) << 16
    _assert_idle_error(
        _package(extra_entries=[("assets/link.png", b"preview.png", mode)]),
        "unsupported_entry",
    )


def test_rejects_duplicate_normalized_paths():
    _assert_idle_error(_package(extra_entries=[("./manifest.json", b"{}")]), "duplicate_path")


def test_rejects_mime_spoofing_hash_schema_and_reference_mismatch():
    bad_asset = _asset("logo", "assets/logo.png", "image", "image/png", b"not a png")
    _assert_idle_error(
        _package(assets=[bad_asset], asset_files={"assets/logo.png": b"not a png"}),
        "mime_mismatch",
    )

    mismatched_hash = _asset(
        "logo", "assets/logo.png", "image", "image/png", PNG_1X1, sha="0" * 64
    )
    _assert_idle_error(
        _package(assets=[mismatched_hash], asset_files={"assets/logo.png": PNG_1X1}),
        "asset_hash_mismatch",
    )

    _assert_idle_error(_package(manifest_overrides={"name": ""}), "manifest_schema")

    layer = {
        "id": "Logo",
        "type": "image",
        "transform": {"x": 0, "y": 0, "width": 100, "height": 100},
        "asset": "missing",
    }
    _assert_idle_error(_package(layers=[layer]), "missing_asset_reference")


def test_rejects_remote_lottie_assets_and_expressions():
    lottie_remote = json.dumps(
        {"v": "5.0.0", "assets": [{"u": "https://example.com/"}]}
    ).encode()
    remote_asset = _asset(
        "anim",
        "assets/anim.json",
        "lottie",
        "application/vnd.meticulous.lottie+json",
        lottie_remote,
    )
    lottie_layer = {
        "id": "Anim",
        "type": "lottie",
        "transform": {"x": 0, "y": 0, "width": 100, "height": 100},
        "asset": "anim",
    }
    _assert_idle_error(
        _package(
            assets=[remote_asset],
            asset_files={"assets/anim.json": lottie_remote},
            layers=[lottie_layer],
        ),
        "lottie_remote_asset",
    )

    lottie_expression = json.dumps(
        {"v": "5.0.0", "layers": [{"ks": {"o": {"x": "time*2"}}}]}
    ).encode()
    expression_asset = _asset(
        "anim",
        "assets/anim.json",
        "lottie",
        "application/vnd.meticulous.lottie+json",
        lottie_expression,
    )
    _assert_idle_error(
        _package(
            assets=[expression_asset],
            asset_files={"assets/anim.json": lottie_expression},
            layers=[lottie_layer],
        ),
        "lottie_expression",
    )


def test_size_and_count_limits(monkeypatch):
    package_bytes = _package()
    monkeypatch.setattr(idle_screens, "MAX_COMPRESSED_BYTES", len(package_bytes) - 1)
    _assert_idle_error(package_bytes, "compressed_size_limit")

    monkeypatch.setattr(idle_screens, "MAX_COMPRESSED_BYTES", 10 * 1024 * 1024)
    monkeypatch.setattr(idle_screens, "MAX_ENTRY_BYTES", 2)
    _assert_idle_error(package_bytes, "entry_size_limit")

    monkeypatch.setattr(idle_screens, "MAX_ENTRY_BYTES", 8 * 1024 * 1024)
    monkeypatch.setattr(idle_screens, "MAX_EXTRACTED_BYTES", 100)
    _assert_idle_error(package_bytes, "extracted_size_limit")

    monkeypatch.setattr(idle_screens, "MAX_EXTRACTED_BYTES", 30 * 1024 * 1024)
    monkeypatch.setattr(idle_screens, "MAX_ENTRIES", 2)
    _assert_idle_error(package_bytes, "entry_count_limit")


def test_raster_dimension_limit():
    oversized_png = bytearray(PNG_1X1)
    oversized_png[16:20] = (1025).to_bytes(4, "big")
    _assert_idle_error(_package(preview_data=bytes(oversized_png)), "raster_size_limit")


def test_interrupted_staging_cleans_up_without_install(monkeypatch, idle_screen_root):
    original_write = idle_screens._write_file_private

    def fail_on_screen(path, data):
        if path.name == "screen.json":
            raise RuntimeError("stop")
        return original_write(path, data)

    monkeypatch.setattr(idle_screens, "_write_file_private", fail_on_screen)
    with pytest.raises(RuntimeError):
        idle_screens.install_package(_package(package_id="custom:broken"))

    assert [path.name for path in idle_screen_root.iterdir()] == [".staging"]
    assert list(idle_screen_root.joinpath(".staging").iterdir()) == []


def test_failed_replacement_rename_restores_current(monkeypatch):
    idle_screens.install_package(_package(package_id="custom:clock", version="1.0.0"))
    original_rename = idle_screens._rename
    failed = False

    def fail_stage_to_target(src, dst):
        nonlocal failed
        if (
            not failed
            and src.name.startswith("custom__clock-")
            and not src.name.endswith(".new-rollback")
            and dst.name == "custom__clock"
        ):
            failed = True
            raise OSError("rename failed")
        return original_rename(src, dst)

    monkeypatch.setattr(idle_screens, "_rename", fail_stage_to_target)
    with pytest.raises(IdleScreenError) as exc:
        idle_screens.install_package(_package(package_id="custom:clock", version="2.0.0"))

    assert exc.value.code == "install_failed"
    assert idle_screens.list_installed_screens()[0]["version"] == "1.0.0"


def test_successful_install_is_not_failed_by_post_rename_fsync(monkeypatch):
    idle_screens.install_package(_package(package_id="custom:clock", version="1.0.0"))
    original_fsync_dir = idle_screens._fsync_dir

    def fail_visible_fsync(path):
        if path.name in {"idle-screens", ".rollback"}:
            raise OSError("fsync failed")
        return original_fsync_dir(path)

    monkeypatch.setattr(idle_screens, "_fsync_dir", fail_visible_fsync)
    screen, replaced = idle_screens.install_package(
        _package(package_id="custom:clock", version="2.0.0")
    )

    assert replaced is True
    assert screen["version"] == "2.0.0"
    assert idle_screens.list_installed_screens()[0]["version"] == "2.0.0"


def test_successful_replacement_is_not_failed_by_old_rollback_cleanup(monkeypatch):
    idle_screens.install_package(_package(package_id="custom:clock", version="1.0.0"))
    idle_screens.install_package(_package(package_id="custom:clock", version="2.0.0"))
    original_rmtree = idle_screens.shutil.rmtree

    def fail_old_rollback_cleanup(path, *args, **kwargs):
        if Path(path).name.endswith(".old-rollback"):
            raise OSError("cleanup failed")
        return original_rmtree(path, *args, **kwargs)

    monkeypatch.setattr(idle_screens.shutil, "rmtree", fail_old_rollback_cleanup)
    screen, replaced = idle_screens.install_package(
        _package(package_id="custom:clock", version="3.0.0")
    )

    assert replaced is True
    assert screen["version"] == "3.0.0"
    assert idle_screens.list_installed_screens()[0]["version"] == "3.0.0"


def test_successful_rollback_is_not_failed_by_post_rename_fsync(monkeypatch):
    idle_screens.install_package(_package(package_id="custom:clock", version="1.0.0"))
    idle_screens.install_package(_package(package_id="custom:clock", version="2.0.0"))
    original_fsync_dir = idle_screens._fsync_dir

    def fail_visible_fsync(path):
        if path.name in {"idle-screens", ".rollback"}:
            raise OSError("fsync failed")
        return original_fsync_dir(path)

    monkeypatch.setattr(idle_screens, "_fsync_dir", fail_visible_fsync)
    screen = idle_screens.rollback_package("custom:clock")

    assert screen["version"] == "1.0.0"
    assert idle_screens.list_installed_screens()[0]["version"] == "1.0.0"


def test_concurrent_same_id_uploads_serialize():
    errors = []

    def upload(version):
        try:
            idle_screens.install_package(_package(package_id="custom:clock", version=version))
        except Exception as exc:
            errors.append(exc)

    threads = [
        threading.Thread(target=upload, args=(version,)) for version in ["1.0.0", "2.0.0"]
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert errors == []
    screen = idle_screens.list_installed_screens()[0]
    assert screen["id"] == "custom:clock"
    assert screen["version"] in {"1.0.0", "2.0.0"}


class TestIdleScreensAPI(AsyncHTTPTestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.previous_root = idle_screens.IDLE_SCREENS_ROOT
        idle_screens.IDLE_SCREENS_ROOT = Path(self.temporary.name).joinpath("idle-screens")
        idle_screens._locks.clear()
        super().setUp()

    def tearDown(self):
        super().tearDown()
        idle_screens.IDLE_SCREENS_ROOT = self.previous_root
        idle_screens._locks.clear()
        self.temporary.cleanup()

    def get_app(self):
        return Application(
            [
                (r"/api/v1/idle-screens", idle_screens.IdleScreensHandler),
                (
                    r"/api/v1/idle-screens/([^/]+)/rollback",
                    idle_screens.IdleScreenRollbackHandler,
                ),
            ]
        )

    def multipart(self, package_bytes):
        boundary = "met-boundary"
        body = (
            (
                f"--{boundary}\r\n"
                'Content-Disposition: form-data; name="file"; filename="screen.metidle"\r\n'
                "Content-Type: application/octet-stream\r\n\r\n"
            ).encode()
            + package_bytes
            + f"\r\n--{boundary}--\r\n".encode()
        )
        return body, {"Content-Type": f"multipart/form-data; boundary={boundary}"}

    def test_upload_list_and_rollback_response_shapes(self):
        body, headers = self.multipart(_package(package_id="custom:api", version="1.0.0"))
        response = self.fetch("/api/v1/idle-screens", method="POST", body=body, headers=headers)
        assert response.code == 200
        payload = json.loads(response.body)
        assert payload["replaced"] is False
        assert payload["screen"]["id"] == "custom:api"

        body, headers = self.multipart(_package(package_id="custom:api", version="2.0.0"))
        response = self.fetch("/api/v1/idle-screens", method="POST", body=body, headers=headers)
        assert json.loads(response.body)["replaced"] is True

        response = self.fetch("/api/v1/idle-screens")
        assert json.loads(response.body)["screens"][0]["rollbackAvailable"] is True

        response = self.fetch(
            "/api/v1/idle-screens/custom:api/rollback", method="POST", body=b""
        )
        payload = json.loads(response.body)
        assert response.code == 200
        assert payload["rolledBack"] is True
        assert payload["screen"]["version"] == "1.0.0"

    def test_structured_api_error(self):
        response = self.fetch("/api/v1/idle-screens", method="POST", body=b"")
        payload = json.loads(response.body)
        assert response.code == 400
        assert payload["error"] == "invalid_idle_screen"
        assert payload["code"] == "missing_file"

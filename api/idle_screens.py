import errno
import hashlib
import json
import os
import posixpath
import re
import shutil
import stat
import struct
import threading
import uuid
import zipfile
from dataclasses import dataclass
from datetime import datetime, timezone
from io import BytesIO
from pathlib import Path, PurePosixPath
from typing import Any

from jsonschema import Draft202012Validator, ValidationError

from log import MeticulousLogger

from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)

IDLE_SCREENS_ROOT = Path(os.getenv("IDLE_SCREENS_PATH", "/meticulous-user/idle-screens"))
SCHEMA_PATH = (
    Path(__file__).resolve().parent.parent.joinpath("schemas", "idle-screen.schema.json")
)
SCHEMA_SHA256 = "db230f72228f08b50f873d79e6a65149d40744e88f51d8a7b6ff772384614d44"

PACKAGE_FORMAT = 1
SCREEN_SCHEMA_VERSION = 2
RUNTIME_API = 1

MAX_COMPRESSED_BYTES = 10 * 1024 * 1024
MAX_EXTRACTED_BYTES = 30 * 1024 * 1024
MAX_ENTRY_BYTES = 8 * 1024 * 1024
MAX_ENTRIES = 128
MAX_RASTER_DIMENSION = 1024

ROOT_FILES = {"manifest.json", "screen.json", "preview.png"}
RESERVED_IDS = {"default", "digital", "metCat", "dvd", "baristaBarista"}
REMOTE_URL_RE = re.compile(r"^(?:https?:)?//|^(?:data|file):", re.IGNORECASE)
CUSTOM_ID_RE = re.compile(r"^custom:[a-z0-9][a-z0-9._-]{0,71}$")

_locks_guard = threading.Lock()
_locks: dict[str, threading.Lock] = {}


class IdleScreenError(Exception):
    def __init__(self, code: str, details: str, status: int = 400):
        super().__init__(details)
        self.code = code
        self.details = details
        self.status = status


@dataclass
class ValidatedPackage:
    package_id: str
    name: str
    version: str
    package_hash: str
    installed_at: str
    files: dict[str, bytes]


def _error(handler: BaseHandler, exc: IdleScreenError):
    handler.set_status(exc.status)
    handler.write(
        {
            "error": "invalid_idle_screen",
            "code": exc.code,
            "details": exc.details,
        }
    )


def _load_schema() -> dict[str, Any]:
    schema_bytes = SCHEMA_PATH.read_bytes()
    digest = hashlib.sha256(schema_bytes).hexdigest()
    if digest != SCHEMA_SHA256:
        raise IdleScreenError(
            "schema_mismatch",
            "Vendored idle screen schema does not match the expected contract.",
            500,
        )
    return json.loads(schema_bytes.decode("utf-8"))


def _schema_validator(definition_name: str) -> Draft202012Validator:
    schema = _load_schema()
    return Draft202012Validator(
        {
            "$schema": schema.get("$schema"),
            "$defs": schema["$defs"],
            "$ref": f"#/$defs/{definition_name}",
        }
    )


def _json_load(data: bytes, code: str) -> dict[str, Any]:
    try:
        value = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise IdleScreenError(code, "Package JSON could not be decoded.") from exc
    if not isinstance(value, dict):
        raise IdleScreenError(code, "Package JSON must be an object.")
    return value


def _validate_schema(definition_name: str, value: dict[str, Any], code: str):
    try:
        _schema_validator(definition_name).validate(value)
    except ValidationError as exc:
        path = ".".join(str(part) for part in exc.absolute_path)
        location = f" at {path}" if path else ""
        raise IdleScreenError(
            code, f"{definition_name}.json failed schema validation{location}."
        )


def _safe_archive_name(name: str) -> str:
    if not name or "\x00" in name or "\\" in name:
        raise IdleScreenError("invalid_path", "Archive contains an unsupported path.")
    if name.startswith("/") or re.match(r"^[A-Za-z]:", name):
        raise IdleScreenError("invalid_path", "Archive contains an absolute path.")
    normalized = posixpath.normpath(name)
    if normalized in {"", "."} or normalized.startswith("../") or normalized == "..":
        raise IdleScreenError("invalid_path", "Archive contains a traversal path.")
    if PurePosixPath(normalized).is_absolute():
        raise IdleScreenError("invalid_path", "Archive contains an absolute path.")
    if any(part in {"", ".", ".."} for part in PurePosixPath(normalized).parts):
        raise IdleScreenError("invalid_path", "Archive contains a traversal path.")
    return normalized


def _is_allowed_archive_member(name: str, is_dir: bool) -> bool:
    if name in ROOT_FILES:
        return not is_dir
    if name == "assets":
        return is_dir
    if name.startswith("assets/"):
        return True
    return False


def _zip_entry_type(info: zipfile.ZipInfo) -> int:
    mode = (info.external_attr >> 16) & 0o170000
    if mode == 0:
        return stat.S_IFDIR if info.is_dir() else stat.S_IFREG
    return stat.S_IFMT(mode)


def _read_zip_files(package_bytes: bytes) -> dict[str, bytes]:
    if len(package_bytes) > MAX_COMPRESSED_BYTES:
        raise IdleScreenError("compressed_size_limit", "Package exceeds compressed size limit.")

    try:
        archive = zipfile.ZipFile(BytesIO(package_bytes), "r")
    except zipfile.BadZipFile as exc:
        raise IdleScreenError("malformed_zip", "Package is not a valid ZIP archive.") from exc

    with archive:
        infos = archive.infolist()
        if len(infos) > MAX_ENTRIES:
            raise IdleScreenError("entry_count_limit", "Package contains too many entries.")

        normalized_names: set[str] = set()
        files: dict[str, bytes] = {}
        total_size = 0

        for info in infos:
            if info.flag_bits & 0x1:
                raise IdleScreenError(
                    "encrypted_archive", "Encrypted archives are not supported."
                )

            name = _safe_archive_name(info.filename)
            folded_name = name.casefold()
            if folded_name in normalized_names:
                raise IdleScreenError("duplicate_path", "Archive contains duplicate paths.")
            normalized_names.add(folded_name)

            entry_type = _zip_entry_type(info)
            if entry_type not in {stat.S_IFREG, stat.S_IFDIR}:
                raise IdleScreenError(
                    "unsupported_entry", "Archive contains an unsupported entry."
                )
            if not _is_allowed_archive_member(name, entry_type == stat.S_IFDIR):
                raise IdleScreenError(
                    "invalid_layout", "Archive contains files outside the idle screen layout."
                )
            if entry_type == stat.S_IFDIR:
                continue
            if info.file_size > MAX_ENTRY_BYTES:
                raise IdleScreenError("entry_size_limit", "Archive entry exceeds size limit.")

            total_size += info.file_size
            if total_size > MAX_EXTRACTED_BYTES:
                raise IdleScreenError(
                    "extracted_size_limit", "Package exceeds extracted size limit."
                )

            try:
                data = archive.read(info)
            except RuntimeError as exc:
                raise IdleScreenError(
                    "encrypted_archive", "Encrypted archives are not supported."
                ) from exc
            if len(data) != info.file_size:
                raise IdleScreenError("malformed_zip", "Archive entry could not be read.")
            files[name] = data

    missing = ROOT_FILES.difference(files.keys())
    if missing:
        raise IdleScreenError("invalid_layout", "Package is missing required files.")
    return files


def _png_dimensions(data: bytes) -> tuple[int, int]:
    if not data.startswith(b"\x89PNG\r\n\x1a\n") or len(data) < 24:
        raise ValueError
    return struct.unpack(">II", data[16:24])


def _jpeg_dimensions(data: bytes) -> tuple[int, int]:
    if not data.startswith(b"\xff\xd8"):
        raise ValueError
    offset = 2
    while offset + 9 <= len(data):
        if data[offset] != 0xFF:
            raise ValueError
        marker = data[offset + 1]
        offset += 2
        while marker == 0xFF and offset < len(data):
            marker = data[offset]
            offset += 1
        if marker in {0xD8, 0xD9} or 0xD0 <= marker <= 0xD7:
            continue
        if offset + 2 > len(data):
            raise ValueError
        segment_length = struct.unpack(">H", data[offset : offset + 2])[0]
        if segment_length < 2 or offset + segment_length > len(data):
            raise ValueError
        if marker in {
            0xC0,
            0xC1,
            0xC2,
            0xC3,
            0xC5,
            0xC6,
            0xC7,
            0xC9,
            0xCA,
            0xCB,
            0xCD,
            0xCE,
            0xCF,
        }:
            height, width = struct.unpack(">HH", data[offset + 3 : offset + 7])
            return width, height
        offset += segment_length
    raise ValueError


def _webp_dimensions(data: bytes) -> tuple[int, int]:
    if len(data) < 30 or data[:4] != b"RIFF" or data[8:12] != b"WEBP":
        raise ValueError
    chunk = data[12:16]
    if chunk == b"VP8X":
        if len(data) < 30:
            raise ValueError
        width = 1 + int.from_bytes(data[24:27], "little")
        height = 1 + int.from_bytes(data[27:30], "little")
        return width, height
    if chunk == b"VP8 ":
        if len(data) < 30 or data[23:26] != b"\x9d\x01\x2a":
            raise ValueError
        width = struct.unpack("<H", data[26:28])[0] & 0x3FFF
        height = struct.unpack("<H", data[28:30])[0] & 0x3FFF
        return width, height
    if chunk == b"VP8L":
        if len(data) < 25 or data[20] != 0x2F:
            raise ValueError
        bits = int.from_bytes(data[21:25], "little")
        width = (bits & 0x3FFF) + 1
        height = ((bits >> 14) & 0x3FFF) + 1
        return width, height
    raise ValueError


def _validate_raster(data: bytes, mime_type: str):
    try:
        if mime_type == "image/png":
            width, height = _png_dimensions(data)
        elif mime_type == "image/jpeg":
            width, height = _jpeg_dimensions(data)
        elif mime_type == "image/webp":
            width, height = _webp_dimensions(data)
        else:
            raise ValueError
    except ValueError as exc:
        raise IdleScreenError(
            "mime_mismatch", "Asset content does not match its MIME type."
        ) from exc
    if width > MAX_RASTER_DIMENSION or height > MAX_RASTER_DIMENSION:
        raise IdleScreenError("raster_size_limit", "Raster asset dimensions exceed limit.")


def _validate_asset_bytes(asset: dict[str, Any], data: bytes):
    if len(data) != asset["size"]:
        raise IdleScreenError("asset_size_mismatch", "Asset size does not match manifest.")
    if hashlib.sha256(data).hexdigest() != asset["sha256"]:
        raise IdleScreenError("asset_hash_mismatch", "Asset hash does not match manifest.")

    mime_type = asset["mimeType"]
    if mime_type.startswith("image/"):
        _validate_raster(data, mime_type)
    elif mime_type == "font/woff2":
        if not data.startswith(b"wOF2"):
            raise IdleScreenError(
                "mime_mismatch", "Asset content does not match its MIME type."
            )
    elif mime_type == "application/vnd.meticulous.lottie+json":
        lottie = _json_load(data, "invalid_lottie")
        _validate_lottie(lottie)
    else:
        raise IdleScreenError("mime_mismatch", "Unsupported asset MIME type.")


def _validate_lottie(value: Any):
    if isinstance(value, dict):
        for key, item in value.items():
            if key == "x" and isinstance(item, str):
                raise IdleScreenError(
                    "lottie_expression", "Lottie expressions are not supported."
                )
            if key.lower() in {"expression", "expr"}:
                raise IdleScreenError(
                    "lottie_expression", "Lottie expressions are not supported."
                )
            if key in {"u", "p"} and isinstance(item, str) and REMOTE_URL_RE.search(item):
                raise IdleScreenError(
                    "lottie_remote_asset", "Remote Lottie assets are not supported."
                )
            _validate_lottie(item)
    elif isinstance(value, list):
        for item in value:
            _validate_lottie(item)
    elif isinstance(value, str) and REMOTE_URL_RE.search(value):
        raise IdleScreenError("lottie_remote_asset", "Remote Lottie assets are not supported.")


def _asset_kind(manifest: dict[str, Any], asset_id: str) -> str | None:
    for asset in manifest["assets"]:
        if asset["id"] == asset_id:
            return asset["kind"]
    return None


def _require_asset(manifest: dict[str, Any], asset_id: Any, expected_kind: str):
    if not isinstance(asset_id, str):
        return
    actual_kind = _asset_kind(manifest, asset_id)
    if actual_kind is None:
        raise IdleScreenError("missing_asset_reference", "Screen references an unknown asset.")
    if actual_kind != expected_kind:
        raise IdleScreenError(
            "asset_kind_mismatch", "Screen references an asset of the wrong kind."
        )


def _validate_font_reference(manifest: dict[str, Any], font: Any):
    if isinstance(font, dict) and "asset" in font:
        _require_asset(manifest, font["asset"], "font")


def _validate_layer_references(manifest: dict[str, Any], layer: dict[str, Any]):
    layer_type = layer.get("type")
    if layer_type == "group":
        for child in layer.get("children", []):
            if isinstance(child, dict):
                _validate_layer_references(manifest, child)
    elif layer_type == "text":
        _validate_font_reference(manifest, layer.get("font"))
    elif layer_type == "digitalTime":
        _validate_font_reference(manifest, layer.get("font"))
        _validate_font_reference(manifest, layer.get("middayFont"))
    elif layer_type == "image":
        _require_asset(manifest, layer.get("asset"), "image")
        for variant in layer.get("variants", []):
            if isinstance(variant, dict):
                _require_asset(manifest, variant.get("asset"), "image")
    elif layer_type == "analogHand" and "asset" in layer:
        _require_asset(manifest, layer.get("asset"), "image")
    elif layer_type == "lottie":
        _require_asset(manifest, layer.get("asset"), "lottie")


def _validate_references(manifest: dict[str, Any], screen: dict[str, Any]):
    for layer in screen.get("layers", []):
        if isinstance(layer, dict):
            _validate_layer_references(manifest, layer)


def _validate_manifest_assets(manifest: dict[str, Any], files: dict[str, bytes]):
    asset_ids: set[str] = set()
    asset_paths: set[str] = set()
    for asset in manifest["assets"]:
        if asset["id"] in asset_ids:
            raise IdleScreenError("duplicate_asset", "Manifest contains duplicate asset IDs.")
        if asset["path"] in asset_paths:
            raise IdleScreenError("duplicate_asset", "Manifest contains duplicate asset paths.")
        asset_ids.add(asset["id"])
        asset_paths.add(asset["path"])

        data = files.get(asset["path"])
        if data is None:
            raise IdleScreenError(
                "missing_asset", "Manifest asset is missing from the package."
            )
        _validate_asset_bytes(asset, data)

    unexpected_assets = [
        path for path in files.keys() if path.startswith("assets/") and path not in asset_paths
    ]
    if unexpected_assets:
        raise IdleScreenError("unapproved_asset", "Package contains an unapproved asset.")


def _validate_ids(manifest: dict[str, Any], screen: dict[str, Any]):
    package_id = manifest["id"]
    if package_id in RESERVED_IDS or not CUSTOM_ID_RE.fullmatch(package_id):
        raise IdleScreenError("reserved_id", "Idle screen packages must use a custom ID.")
    if screen["id"] != package_id:
        raise IdleScreenError("id_mismatch", "Manifest and screen IDs do not match.")
    if manifest["packageFormat"] != PACKAGE_FORMAT:
        raise IdleScreenError("unsupported_package_format", "Unsupported package format.")
    if screen["schemaVersion"] != SCREEN_SCHEMA_VERSION:
        raise IdleScreenError("unsupported_screen_schema", "Unsupported screen schema version.")
    if manifest["runtimeApi"] != RUNTIME_API or screen["runtimeApi"] != RUNTIME_API:
        raise IdleScreenError("unsupported_runtime_api", "Unsupported runtime API version.")


def validate_package(package_bytes: bytes) -> ValidatedPackage:
    files = _read_zip_files(package_bytes)
    manifest = _json_load(files["manifest.json"], "invalid_manifest_json")
    screen = _json_load(files["screen.json"], "invalid_screen_json")
    _validate_schema("manifest", manifest, "manifest_schema")
    _validate_schema("screen", screen, "screen_schema")
    _validate_ids(manifest, screen)
    _validate_raster(files["preview.png"], "image/png")
    _validate_manifest_assets(manifest, files)
    _validate_references(manifest, screen)

    return ValidatedPackage(
        package_id=manifest["id"],
        name=manifest["name"],
        version=manifest["version"],
        package_hash=hashlib.sha256(package_bytes).hexdigest(),
        installed_at=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        files=files,
    )


def _safe_id_path_name(package_id: str) -> str:
    return package_id


def _lock_for(package_id: str) -> threading.Lock:
    with _locks_guard:
        if package_id not in _locks:
            _locks[package_id] = threading.Lock()
        return _locks[package_id]


def _ensure_private_dir(path: Path):
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)


def _fsync_dir(path: Path):
    try:
        fd = os.open(path, os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(fd)
    except OSError as exc:
        if exc.errno not in {errno.EINVAL, errno.ENOTSUP}:
            raise
    finally:
        os.close(fd)


def _fsync_dir_best_effort(path: Path, context: str):
    try:
        _fsync_dir(path)
    except OSError as exc:
        logger.warning(
            f"Idle screen directory fsync failed after {context}: {type(exc).__name__}"
        )


def _cleanup_tree_best_effort(path: Path, context: str):
    try:
        shutil.rmtree(path)
    except FileNotFoundError:
        return
    except Exception as exc:
        logger.warning(f"Idle screen cleanup failed after {context}: {type(exc).__name__}")


def _write_file_private(path: Path, data: bytes):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.parent.chmod(0o700)
    with path.open("wb") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())
    path.chmod(0o600)


def _fsync_tree_dirs(root: Path):
    dirs = sorted(
        [p for p in root.rglob("*") if p.is_dir()],
        key=lambda p: len(p.parts),
        reverse=True,
    )
    for path in dirs:
        _fsync_dir(path)
    _fsync_dir(root)


def _stage_package(package: ValidatedPackage) -> Path:
    _ensure_private_dir(IDLE_SCREENS_ROOT)
    staging_root = IDLE_SCREENS_ROOT.joinpath(".staging")
    _ensure_private_dir(staging_root)
    stage = staging_root.joinpath(
        f"{_safe_id_path_name(package.package_id)}-{uuid.uuid4().hex}"
    )
    stage.mkdir(mode=0o700)

    try:
        for relative_name, data in package.files.items():
            _write_file_private(stage.joinpath(relative_name), data)
        _write_file_private(
            stage.joinpath(".install.json"),
            json.dumps(
                {
                    "id": package.package_id,
                    "name": package.name,
                    "version": package.version,
                    "packageHash": package.package_hash,
                    "installedAt": package.installed_at,
                },
                sort_keys=True,
            ).encode("utf-8"),
        )
        _fsync_tree_dirs(stage)
        _fsync_dir(staging_root)
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise
    return stage


def _package_paths(package_id: str) -> tuple[Path, Path]:
    safe_name = _safe_id_path_name(package_id)
    return IDLE_SCREENS_ROOT.joinpath(safe_name), IDLE_SCREENS_ROOT.joinpath(
        ".rollback", safe_name
    )


def _rename(src: Path, dst: Path):
    os.replace(src, dst)
    _fsync_dir_best_effort(dst.parent, "rename")


def _install_validated_package(package: ValidatedPackage) -> tuple[dict[str, Any], bool]:
    target, rollback = _package_paths(package.package_id)
    stage = _stage_package(package)
    rollback_root = rollback.parent
    _ensure_private_dir(rollback_root)

    lock = _lock_for(package.package_id)
    with lock:
        replaced = target.exists()
        rollback_old = Path(f"{stage}.old-rollback")
        rollback_new = Path(f"{stage}.new-rollback")
        rollback_old_moved = False
        target_moved = False
        stage_installed = False

        try:
            if rollback.exists():
                _rename(rollback, rollback_old)
                rollback_old_moved = True
            if target.exists():
                _rename(target, rollback_new)
                target_moved = True
            _rename(stage, target)
            stage_installed = True
            if rollback_new.exists():
                _rename(rollback_new, rollback)
            if rollback_old.exists():
                _cleanup_tree_best_effort(rollback_old, "replacement")
            _fsync_dir_best_effort(IDLE_SCREENS_ROOT, "replacement")
            screen = installed_screen_from_dir(target, rollback.exists())
            return screen, replaced
        except Exception as exc:
            recovery_error = None
            try:
                if stage_installed and target.exists():
                    shutil.rmtree(target)
                if target_moved and rollback_new.exists() and not target.exists():
                    _rename(rollback_new, target)
                if rollback_old_moved and rollback_old.exists() and not rollback.exists():
                    _rename(rollback_old, rollback)
            except Exception as recovery_exc:
                recovery_error = recovery_exc
            if stage.exists():
                _cleanup_tree_best_effort(stage, "failed replacement")
            logger.warning("Idle screen installation failed; previous install restored")
            if recovery_error is not None:
                raise IdleScreenError(
                    "install_recovery_failed",
                    "Idle screen installation failed and recovery was incomplete.",
                    500,
                ) from recovery_error
            if isinstance(exc, IdleScreenError):
                raise
            raise IdleScreenError(
                "install_failed", "Idle screen installation failed.", 500
            ) from exc


def installed_screen_from_dir(path: Path, rollback_available: bool) -> dict[str, Any]:
    metadata: dict[str, Any] = {}
    manifest: dict[str, Any] = {}
    try:
        metadata = json.loads(path.joinpath(".install.json").read_text(encoding="utf-8"))
    except Exception:
        metadata = {}
    try:
        manifest = json.loads(path.joinpath("manifest.json").read_text(encoding="utf-8"))
    except Exception:
        manifest = {}

    screen = {
        "id": str(metadata.get("id") or manifest.get("id") or path.name),
        "name": str(metadata.get("name") or manifest.get("name") or path.name),
        "version": str(metadata.get("version") or manifest.get("version") or ""),
        "packageHash": str(metadata.get("packageHash") or ""),
        "installedAt": str(metadata.get("installedAt") or _mtime_iso(path)),
        "rollbackAvailable": rollback_available,
        "valid": False,
    }

    try:
        files = _read_installed_files(path)
        manifest = _json_load(files["manifest.json"], "invalid_manifest_json")
        screen_json = _json_load(files["screen.json"], "invalid_screen_json")
        _validate_schema("manifest", manifest, "manifest_schema")
        _validate_schema("screen", screen_json, "screen_schema")
        _validate_ids(manifest, screen_json)
        _validate_raster(files["preview.png"], "image/png")
        _validate_manifest_assets(manifest, files)
        _validate_references(manifest, screen_json)
        for field in ("id", "name", "version", "packageHash", "installedAt"):
            if not metadata.get(field):
                raise IdleScreenError("metadata_mismatch", "Installed metadata is incomplete.")
        for field in ("id", "name", "version"):
            if metadata[field] != manifest[field]:
                raise IdleScreenError(
                    "metadata_mismatch", "Installed metadata does not match manifest."
                )
        screen.update(
            {
                "id": str(manifest["id"]),
                "name": str(manifest["name"]),
                "version": str(manifest["version"]),
                "valid": True,
            }
        )
        return screen
    except Exception:
        return screen


def _read_installed_files(path: Path) -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    for member in path.rglob("*"):
        relative_name = member.relative_to(path).as_posix()
        if relative_name == ".install.json":
            continue
        if member.is_dir():
            continue
        if not member.is_file():
            raise IdleScreenError(
                "unsupported_entry", "Installed package contains an unsupported entry."
            )
        if not _is_allowed_archive_member(relative_name, False):
            raise IdleScreenError(
                "invalid_layout",
                "Installed package contains files outside the idle screen layout.",
            )
        files[relative_name] = member.read_bytes()

    missing = ROOT_FILES.difference(files.keys())
    if missing:
        raise IdleScreenError("invalid_layout", "Installed package is missing required files.")
    return files


def _mtime_iso(path: Path) -> str:
    return (
        datetime.fromtimestamp(path.stat().st_mtime, timezone.utc)
        .isoformat()
        .replace("+00:00", "Z")
    )


def list_installed_screens() -> list[dict[str, Any]]:
    _ensure_private_dir(IDLE_SCREENS_ROOT)
    rollback_root = IDLE_SCREENS_ROOT.joinpath(".rollback")
    screens = []
    for path in sorted(IDLE_SCREENS_ROOT.iterdir(), key=lambda p: p.name):
        if not path.is_dir() or path.name.startswith("."):
            continue
        screens.append(
            installed_screen_from_dir(path, rollback_root.joinpath(path.name).exists())
        )
    return screens


def get_installed_bundle(package_id: str) -> bytes:
    if package_id in RESERVED_IDS or not CUSTOM_ID_RE.fullmatch(package_id):
        raise IdleScreenError("invalid_id", "Idle screen ID is invalid.", 400)
    target, _ = _package_paths(package_id)
    lock = _lock_for(package_id)
    with lock:
        if not target.is_dir():
            raise IdleScreenError("bundle_not_found", "Idle screen bundle was not found.", 404)
        files = _read_installed_files(target)
        installed = installed_screen_from_dir(target, False)
        if not installed["valid"]:
            raise IdleScreenError(
                "invalid_installed_bundle", "Installed idle screen bundle is invalid.", 500
            )

        output = BytesIO()
        with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
            for name in sorted(files):
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o100600 << 16
                archive.writestr(info, files[name])
        return output.getvalue()


def install_package(package_bytes: bytes) -> tuple[dict[str, Any], bool]:
    package = validate_package(package_bytes)
    return _install_validated_package(package)


def rollback_package(package_id: str) -> dict[str, Any]:
    if package_id in RESERVED_IDS or not CUSTOM_ID_RE.fullmatch(package_id):
        raise IdleScreenError("invalid_id", "Idle screen ID is invalid.", 400)
    target, rollback = _package_paths(package_id)
    if not rollback.exists():
        raise IdleScreenError("rollback_unavailable", "Rollback is not available.", 404)

    lock = _lock_for(package_id)
    with lock:
        swap = IDLE_SCREENS_ROOT.joinpath(
            ".staging", f"{_safe_id_path_name(package_id)}-swap-{uuid.uuid4().hex}"
        )
        _ensure_private_dir(swap.parent)
        target_moved = False
        rollback_moved = False
        try:
            if target.exists():
                _rename(target, swap)
                target_moved = True
            _rename(rollback, target)
            rollback_moved = True
            if swap.exists():
                _rename(swap, rollback)
            _fsync_dir_best_effort(IDLE_SCREENS_ROOT, "rollback")
            return installed_screen_from_dir(target, rollback.exists())
        except Exception as exc:
            recovery_error = None
            try:
                if rollback_moved and target.exists() and not rollback.exists():
                    _rename(target, rollback)
                if target_moved and swap.exists() and not target.exists():
                    _rename(swap, target)
            except Exception as recovery_exc:
                recovery_error = recovery_exc
            logger.warning("Idle screen rollback failed; previous install restored")
            if recovery_error is not None:
                raise IdleScreenError(
                    "rollback_recovery_failed",
                    "Idle screen rollback failed and recovery was incomplete.",
                    500,
                ) from recovery_error
            raise IdleScreenError(
                "rollback_failed", "Idle screen rollback failed.", 500
            ) from exc


class IdleScreensHandler(BaseHandler):
    def get(self):
        self.write({"screens": list_installed_screens()})

    def post(self):
        if "file" not in self.request.files or not self.request.files["file"]:
            _error(self, IdleScreenError("missing_file", "Multipart field 'file' is required."))
            return
        fileinfo = self.request.files["file"][0]
        try:
            screen, replaced = install_package(fileinfo["body"])
        except IdleScreenError as exc:
            _error(self, exc)
            return
        self.write({"screen": screen, "replaced": replaced})


class IdleScreenRollbackHandler(BaseHandler):
    def post(self, package_id: str):
        try:
            screen = rollback_package(package_id)
        except IdleScreenError as exc:
            _error(self, exc)
            return
        self.write({"screen": screen, "rolledBack": True})


class IdleScreenBundleHandler(BaseHandler):
    def get(self, package_id: str):
        try:
            bundle = get_installed_bundle(package_id)
        except IdleScreenError as exc:
            _error(self, exc)
            return
        safe_filename = _safe_id_path_name(package_id)
        self.set_header("Content-Type", "application/vnd.meticulous.idle-screen")
        self.set_header(
            "Content-Disposition", f'attachment; filename="{safe_filename}.metidle"'
        )
        self.write(bundle)


API.register_handler(APIVersion.V1, r"/idle-screens", IdleScreensHandler)
API.register_handler(APIVersion.V1, r"/idle-screens/([^/]+)", IdleScreenBundleHandler)
API.register_handler(
    APIVersion.V1, r"/idle-screens/([^/]+)/rollback", IdleScreenRollbackHandler
)

"""Opt-in, anonymous upload of debug shot files to a Supabase storage bucket.

The upload only happens when the user enabled ``shot_data_sharing`` in the
settings. The uploaded copy of the debug file is stripped of everything that
would tie it to a machine or its owner (serial number, hostname, device name,
...) and is grouped under a random per-machine sharing id that is generated
when the user opts in.
"""

import copy
import json
import os
import uuid
from datetime import datetime
from pathlib import Path
from urllib.parse import quote

import aiohttp
import sentry_sdk
import zstandard as zstd

from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    CONFIG_WIFI,
    HOSTNAME_OVERRIDE,
    MeticulousConfig,
    SHOT_DATA_SHARING,
    SHOT_DATA_SHARING_ID,
)
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

# The publishable key is a public client key (like the Sentry DSN); the bucket
# policy only allows anonymous INSERT of .zst files under a UUID folder.
# Set the env vars to point a development machine at a different project.
PLACEHOLDER_MARKER = "PLACEHOLDER"
SUPABASE_URL = os.getenv(
    "SHOT_DATA_SHARING_SUPABASE_URL", "https://szlveuxkohdjujuojwvf.supabase.co"
)
SUPABASE_BUCKET = os.getenv("SHOT_DATA_SHARING_SUPABASE_BUCKET", "brew shots")
SUPABASE_ANON_KEY = os.getenv(
    "SHOT_DATA_SHARING_SUPABASE_ANON_KEY",
    "sb_publishable_5hDHxnYhRLEUeA8wd9m3EQ_SQy9Ks5_",
)

UPLOAD_TIMEOUT_SECONDS = 60
COMPRESSION_LEVEL = 10
SHARED_SHOT_TYPE = "shot"

# Keys of the debug file "machine" section that identify the machine or its owner.
MACHINE_IDENTIFYING_KEYS = (
    "name",
    "hostname",
    "serial_number",
    "batch_number",
    "build_date",
    "color",
)
# Keys of the debug file "config" section that identify the machine or its owner.
CONFIG_IDENTIFYING_KEYS = (HOSTNAME_OVERRIDE, "machine_name", CONFIG_WIFI)


class ShotDataUploadError(Exception):
    """Raised when the storage service rejects an upload."""


class ShotDataSharing:
    @staticmethod
    def is_enabled() -> bool:
        return MeticulousConfig[CONFIG_USER].get(SHOT_DATA_SHARING) is True

    @staticmethod
    def is_configured() -> bool:
        return (
            bool(SUPABASE_URL)
            and bool(SUPABASE_BUCKET)
            and bool(SUPABASE_ANON_KEY)
            and PLACEHOLDER_MARKER not in SUPABASE_URL
            and PLACEHOLDER_MARKER not in SUPABASE_ANON_KEY
        )

    @staticmethod
    def should_upload(shot_type: str) -> bool:
        """Only real (non emulated) brews are shared, and only when opted in."""
        if not ShotDataSharing.is_enabled():
            return False
        if shot_type != SHARED_SHOT_TYPE:
            return False
        from machine import Machine

        if Machine.emulated:
            logger.info("Not sharing emulated debug shots")
            return False
        return True

    @staticmethod
    def generate_sharing_id() -> str:
        return str(uuid.uuid4())

    @staticmethod
    def get_sharing_id() -> str:
        """Return the sharing id, generating and persisting one if missing."""
        sharing_id = MeticulousConfig[CONFIG_SYSTEM].get(SHOT_DATA_SHARING_ID)
        if not isinstance(sharing_id, str) or sharing_id == "":
            sharing_id = ShotDataSharing.generate_sharing_id()
            MeticulousConfig[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] = sharing_id
            MeticulousConfig.save()
            logger.info("Generated a new shot data sharing id")
        return sharing_id

    @staticmethod
    def on_setting_changed(enabled: bool) -> None:
        """Rotate the sharing id on opt-in and forget it on opt-out.

        Called from the settings API before the new value is persisted; the
        caller saves the config afterwards.
        """
        previously_enabled = ShotDataSharing.is_enabled()
        if enabled and not previously_enabled:
            MeticulousConfig[CONFIG_SYSTEM][
                SHOT_DATA_SHARING_ID
            ] = ShotDataSharing.generate_sharing_id()
            logger.info("User opted in to shot data sharing")
        elif not enabled and previously_enabled:
            MeticulousConfig[CONFIG_SYSTEM][SHOT_DATA_SHARING_ID] = None
            logger.info("User opted out of shot data sharing")

    @staticmethod
    def anonymize(debug_shot: dict) -> dict:
        """Return a copy of the debug shot without machine or owner identifiers."""
        anonymized = copy.deepcopy(debug_shot)
        machine = anonymized.get("machine")
        if isinstance(machine, dict):
            for key in MACHINE_IDENTIFYING_KEYS:
                machine.pop(key, None)
        config = anonymized.get("config")
        if isinstance(config, dict):
            for key in CONFIG_IDENTIFYING_KEYS:
                config.pop(key, None)
        return anonymized

    @staticmethod
    def build_object_path(sharing_id: str, start: datetime, file_path: Path) -> str:
        return f"{sharing_id}/{start.strftime('%Y-%m-%d')}/{Path(file_path).name}"

    @staticmethod
    def build_upload_url(object_path: str) -> str:
        # Bucket names may contain spaces; encode every path segment explicitly.
        bucket = quote(SUPABASE_BUCKET, safe="")
        return f"{SUPABASE_URL.rstrip('/')}/storage/v1/object/{bucket}/{quote(object_path, safe='/')}"

    @staticmethod
    def compress(payload: str) -> bytes:
        return zstd.ZstdCompressor(level=COMPRESSION_LEVEL).compress(payload.encode("utf-8"))

    @staticmethod
    def build_headers() -> dict:
        headers = {
            "apikey": SUPABASE_ANON_KEY,
            "Content-Type": "application/zstd",
            "x-upsert": "false",
        }
        # Legacy anon keys are JWTs and go in both headers. The newer
        # sb_publishable_* keys are not JWTs and Storage rejects them as a
        # bearer token ("Invalid Compact JWS"), so they only go in "apikey".
        if SUPABASE_ANON_KEY.startswith("eyJ"):
            headers["Authorization"] = f"Bearer {SUPABASE_ANON_KEY}"
        return headers

    @staticmethod
    async def _post(url: str, body: bytes) -> None:
        headers = ShotDataSharing.build_headers()
        timeout = aiohttp.ClientTimeout(total=UPLOAD_TIMEOUT_SECONDS)
        async with aiohttp.ClientSession(timeout=timeout) as session:
            async with session.post(url, data=body, headers=headers) as response:
                if response.status >= 400:
                    details = (await response.text())[:200]
                    raise ShotDataUploadError(
                        f"Storage service answered HTTP {response.status}: {details}"
                    )

    @staticmethod
    async def upload_debug_shot(data_json: str, start: datetime, file_path: Path) -> bool:
        """Upload an anonymized copy of a debug shot.

        Returns True on success. Failures are logged and reported to Sentry, but
        never raised, so the local debug file handling is unaffected.
        """
        if not ShotDataSharing.is_configured():
            logger.warning(
                "Shot data sharing is enabled but the upload target is not configured"
            )
            return False

        object_path = None
        try:
            debug_shot = json.loads(data_json)
            anonymized = ShotDataSharing.anonymize(debug_shot)
            body = ShotDataSharing.compress(json.dumps(anonymized, ensure_ascii=False))
            object_path = ShotDataSharing.build_object_path(
                ShotDataSharing.get_sharing_id(), start, file_path
            )
            url = ShotDataSharing.build_upload_url(object_path)
            logger.info(f"Uploading shared shot data ({len(body)} bytes) to {object_path}")
            await ShotDataSharing._post(url, body)
        except Exception as e:
            logger.error(
                f"Failed to upload shared shot data {object_path}: {type(e).__name__}: {e}"
            )
            with sentry_sdk.new_scope() as scope:
                scope.set_tag("shot_data_sharing", "upload_failed")
                scope.set_context(
                    "shot_data_sharing",
                    {"object_path": object_path, "bucket": SUPABASE_BUCKET},
                )
                sentry_sdk.capture_exception(e)
            return False

        logger.info(f"Shared shot data uploaded as {object_path}")
        return True

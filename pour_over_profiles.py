import base64
import binascii
import copy
import json
import math
import os
import shutil
import tempfile
import uuid
from pathlib import Path
from typing import Any

import jsonschema

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

POUR_OVER_PROFILE_PATH = Path(
    os.getenv("POUR_OVER_PROFILE_PATH", "/meticulous-user/pour-over-profiles")
)
POUR_OVER_PROFILE_SCHEMA_PATH = Path(__file__).parent.joinpath(
    "pour_over_profile_schema/schema.json"
)
MAX_POUR_OVER_PROFILE_BYTES = 512 * 1024
MAX_EMBEDDED_PROFILE_IMAGE_BYTES = 300 * 1024
MAX_POUR_OVER_PROFILES = 100
MIN_FREE_PROFILE_BYTES = 32 * 1024 * 1024


class PourOverProfileError(Exception):
    pass


class PourOverProfileValidationError(PourOverProfileError):
    def __init__(self, issues: list[dict[str, Any]]):
        super().__init__("Invalid Pour Over profile")
        self.issues = issues


class PourOverProfileTooLargeError(PourOverProfileError):
    pass


class PourOverProfileLimitError(PourOverProfileError):
    pass


class PourOverProfileStorageError(PourOverProfileError):
    pass


class PourOverProfileUnavailableError(PourOverProfileError):
    pass


def _issue(path: list[str | int], message: str, code: str = "invalid") -> dict[str, Any]:
    return {"path": path, "message": message, "code": code}


def _reject_non_finite(value: Any, path: list[str | int], issues: list[dict[str, Any]]):
    if isinstance(value, bool) or value is None:
        return
    if isinstance(value, (int, float)):
        if not math.isfinite(value):
            issues.append(_issue(path, "Number must be finite", "not_finite"))
        return
    if isinstance(value, list):
        for index, child in enumerate(value):
            _reject_non_finite(child, [*path, index], issues)
        return
    if isinstance(value, dict):
        for key, child in value.items():
            _reject_non_finite(child, [*path, key], issues)


class PourOverProfileManager:
    _known_profiles: dict[str, dict[str, Any]] = {}
    _schema: dict[str, Any] | None = None
    _validator: jsonschema.Draft202012Validator | None = None
    _available = False

    @classmethod
    def init(cls):
        try:
            cls._ensure_validator()
            cls._refresh_profile_list()
            cls._available = True
        except Exception as error:
            # Pour Over is optional to the espresso control path. A missing
            # schema or unavailable user partition must never prevent the
            # backend (and therefore espresso) from starting.
            cls._known_profiles = {}
            cls._available = False
            logger.exception(
                "Pour Over profiles are unavailable",
                exc_info=error,
            )

    @classmethod
    def is_available(cls) -> bool:
        return cls._available

    @classmethod
    def _require_available(cls):
        if not cls._available:
            raise PourOverProfileUnavailableError("Pour Over profiles are unavailable")

    @classmethod
    def _ensure_validator(cls):
        if cls._validator is not None:
            return
        with POUR_OVER_PROFILE_SCHEMA_PATH.open("r", encoding="utf-8") as schema_file:
            cls._schema = json.load(schema_file)
        jsonschema.Draft202012Validator.check_schema(cls._schema)
        cls._validator = jsonschema.Draft202012Validator(
            cls._schema,
            format_checker=jsonschema.FormatChecker(),
        )

    @classmethod
    def _ensure_directory(cls):
        POUR_OVER_PROFILE_PATH.mkdir(mode=0o755, parents=True, exist_ok=True)

    @staticmethod
    def _without_image(profile: dict[str, Any]) -> dict[str, Any]:
        summary = copy.deepcopy(profile)
        summary.get("display", {}).pop("image", None)
        return summary

    @classmethod
    def _validate_image(cls, profile: dict[str, Any], issues: list[dict[str, Any]]):
        image = profile.get("display", {}).get("image")
        if image is None:
            return
        prefix = "data:image/jpeg;base64,"
        if not image.startswith(prefix):
            return
        try:
            decoded = base64.b64decode(image[len(prefix) :], validate=True)
        except (binascii.Error, ValueError):
            issues.append(
                _issue(["display", "image"], "Image must contain valid base64", "image_base64")
            )
            return
        if len(decoded) > MAX_EMBEDDED_PROFILE_IMAGE_BYTES:
            issues.append(
                _issue(
                    ["display", "image"],
                    "Embedded image is too large",
                    "image_too_large",
                )
            )
        if (
            len(decoded) < 4
            or not decoded.startswith(b"\xff\xd8")
            or not decoded.endswith(b"\xff\xd9")
        ):
            issues.append(
                _issue(["display", "image"], "Embedded image must be a JPEG", "image_format")
            )

    @classmethod
    def _validate_semantics(cls, profile: dict[str, Any]) -> list[dict[str, Any]]:  # noqa: C901
        issues: list[dict[str, Any]] = []
        _reject_non_finite(profile, [], issues)
        if issues:
            return issues

        for path in (["name"], ["author"]):
            value = profile.get(path[0])
            if isinstance(value, str) and not value.strip():
                issues.append(_issue(path, "Text cannot be blank", "blank"))

        recipe = profile.get("recipe")
        stages = profile.get("stages")
        if not isinstance(recipe, dict) or not isinstance(stages, list) or not stages:
            return issues

        target_time = recipe["target_total_time_s"]
        target_time_max = recipe.get("target_total_time_max_s")
        if target_time_max is not None and target_time_max < target_time:
            issues.append(
                _issue(
                    ["recipe", "target_total_time_max_s"],
                    "Upper target time cannot be earlier than the target time",
                    "time_range",
                )
            )

        keys: set[str] = set()
        cumulative_water = 0.0
        previous_end = 0.0
        for index, stage in enumerate(stages):
            key = stage["key"]
            if not key.strip():
                issues.append(_issue(["stages", index, "key"], "Key cannot be blank", "blank"))
            if not stage["name"].strip():
                issues.append(
                    _issue(["stages", index, "name"], "Name cannot be blank", "blank")
                )
            if key in keys:
                issues.append(
                    _issue(["stages", index, "key"], "Stage keys must be unique", "duplicate")
                )
            keys.add(key)

            starts_at = stage["starts_at_s"]
            if index == 0 and starts_at != 0:
                issues.append(
                    _issue(
                        ["stages", index, "starts_at_s"],
                        "The first pour must start at 0 seconds",
                        "first_start",
                    )
                )
            if index > 0 and starts_at + 0.001 < previous_end:
                issues.append(
                    _issue(
                        ["stages", index, "starts_at_s"],
                        "A pour cannot start before the previous pour ends",
                        "overlap",
                    )
                )

            pour = stage["pour"]
            water = pour["water_g"]
            duration = pour["duration_s"]
            cumulative_water += water
            expected_flow = water / duration
            target_flow = pour["flow_rate_g_s"]
            flow_tolerance = max(0.1, expected_flow * 0.02)
            if abs(target_flow - expected_flow) > flow_tolerance:
                issues.append(
                    _issue(
                        ["stages", index, "pour", "flow_rate_g_s"],
                        f"Flow must equal water divided by duration ({expected_flow:.2f} g/s)",
                        "flow_consistency",
                    )
                )
            if abs(pour["target_cumulative_water_g"] - cumulative_water) > 0.1:
                issues.append(
                    _issue(
                        ["stages", index, "pour", "target_cumulative_water_g"],
                        f"Cumulative target must be {cumulative_water:.1f} g",
                        "water_consistency",
                    )
                )
            flow_range = pour.get("flow_range_g_s")
            if flow_range is not None:
                low, high = flow_range
                if low > high:
                    issues.append(
                        _issue(
                            ["stages", index, "pour", "flow_range_g_s"],
                            "Flow range must be ordered",
                            "flow_range",
                        )
                    )
                elif target_flow < low or target_flow > high:
                    issues.append(
                        _issue(
                            ["stages", index, "pour", "flow_range_g_s"],
                            "Flow target must sit inside the flow range",
                            "flow_range",
                        )
                    )
            previous_end = starts_at + duration

        if abs(cumulative_water - recipe["total_water_g"]) > 0.1:
            issues.append(
                _issue(
                    ["recipe", "total_water_g"],
                    f"Total water must equal the {cumulative_water:.1f} g across all pours",
                    "water_consistency",
                )
            )
        if previous_end > target_time + 0.001:
            issues.append(
                _issue(
                    ["recipe", "target_total_time_s"],
                    "Target brew time cannot end before the final pour",
                    "time_consistency",
                )
            )

        cls._validate_image(profile, issues)
        return issues

    @classmethod
    def validate_profile(cls, value: Any) -> dict[str, Any]:
        cls._ensure_validator()
        if not isinstance(value, dict):
            raise PourOverProfileValidationError([_issue([], "Profile must be an object")])

        issues = [
            _issue(list(error.absolute_path), error.message, "schema")
            for error in sorted(
                cls._validator.iter_errors(value),
                key=lambda error: (
                    tuple(str(part) for part in error.absolute_path),
                    error.message,
                ),
            )
        ]
        if not issues:
            issues.extend(cls._validate_semantics(value))
        if issues:
            raise PourOverProfileValidationError(issues)
        return copy.deepcopy(value)

    @classmethod
    def _refresh_profile_list(cls):
        cls._ensure_directory()
        for temporary_path in POUR_OVER_PROFILE_PATH.glob(".*.tmp"):
            try:
                temporary_path.unlink()
            except OSError as error:
                logger.warning(
                    "Could not remove stale Pour Over temporary file %s: %s",
                    temporary_path.name,
                    error.__class__.__name__,
                )
        valid: dict[str, dict[str, Any]] = {}
        for path in sorted(POUR_OVER_PROFILE_PATH.glob("*.json")):
            try:
                if path.stat().st_size > MAX_POUR_OVER_PROFILE_BYTES:
                    raise PourOverProfileTooLargeError()
                with path.open("r", encoding="utf-8") as profile_file:
                    profile = json.load(
                        profile_file,
                        parse_constant=lambda value: (_ for _ in ()).throw(
                            ValueError(f"Invalid number {value}")
                        ),
                    )
                profile = cls.validate_profile(profile)
                if path.stem != profile["id"]:
                    raise PourOverProfileValidationError(
                        [_issue(["id"], "Stored filename does not match profile id")]
                    )
                valid[profile["id"]] = cls._without_image(profile)
            except Exception as error:
                logger.warning(
                    "Skipping invalid stored Pour Over profile %s: %s",
                    path.name,
                    error.__class__.__name__,
                )
        cls._known_profiles = valid

    @classmethod
    def refresh_profile_list(cls):
        cls._require_available()
        cls._refresh_profile_list()

    @classmethod
    def list_profiles(cls) -> list[dict[str, Any]]:
        cls._require_available()
        return copy.deepcopy(
            sorted(
                cls._known_profiles.values(),
                key=lambda item: (item["name"].casefold(), item["id"]),
            )
        )

    @classmethod
    def get_profile(cls, profile_id: str, include_image: bool = True) -> dict[str, Any] | None:
        cls._require_available()
        profile = cls._known_profiles.get(profile_id)
        if profile is None:
            return None
        if not include_image:
            return copy.deepcopy(profile)

        path = POUR_OVER_PROFILE_PATH.joinpath(f"{profile_id}.json")
        try:
            if path.stat().st_size > MAX_POUR_OVER_PROFILE_BYTES:
                raise PourOverProfileTooLargeError()
            with path.open("r", encoding="utf-8") as profile_file:
                stored = json.load(
                    profile_file,
                    parse_constant=lambda value: (_ for _ in ()).throw(
                        ValueError(f"Invalid number {value}")
                    ),
                )
            stored = cls.validate_profile(stored)
            if stored["id"] != profile_id:
                raise PourOverProfileValidationError(
                    [_issue(["id"], "Stored filename does not match profile id")]
                )
            return stored
        except Exception as error:
            logger.warning(
                "Could not read stored Pour Over profile %s: %s",
                profile_id,
                error.__class__.__name__,
            )
            raise PourOverProfileStorageError("Could not read profile") from error

    @classmethod
    def save_profile(cls, value: Any, change_id: str | None = None) -> dict[str, Any]:
        cls._require_available()
        profile = cls.validate_profile(value)
        serialized = json.dumps(
            profile,
            ensure_ascii=False,
            allow_nan=False,
            indent=2,
        ).encode("utf-8")
        if len(serialized) > MAX_POUR_OVER_PROFILE_BYTES:
            raise PourOverProfileTooLargeError()

        cls._ensure_directory()
        is_update = profile["id"] in cls._known_profiles
        if not is_update and len(cls._known_profiles) >= MAX_POUR_OVER_PROFILES:
            raise PourOverProfileLimitError()
        required_free = MIN_FREE_PROFILE_BYTES + len(serialized)
        if shutil.disk_usage(POUR_OVER_PROFILE_PATH).free < required_free:
            raise PourOverProfileStorageError("Not enough free storage")

        target = POUR_OVER_PROFILE_PATH.joinpath(f"{profile['id']}.json")
        temporary_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="wb",
                prefix=f".{profile['id']}.",
                suffix=".tmp",
                dir=POUR_OVER_PROFILE_PATH,
                delete=False,
            ) as temporary:
                temporary.write(serialized)
                temporary.flush()
                os.fsync(temporary.fileno())
                temporary_path = Path(temporary.name)
            os.replace(temporary_path, target)
            directory_fd = os.open(POUR_OVER_PROFILE_PATH, os.O_RDONLY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
        except Exception as error:
            if temporary_path is not None:
                temporary_path.unlink(missing_ok=True)
            raise PourOverProfileStorageError("Could not save profile") from error

        cls._known_profiles[profile["id"]] = cls._without_image(profile)
        change_id = change_id or str(uuid.uuid4())
        cls._emit_change("update" if is_update else "create", profile["id"], change_id)
        logger.info("Saved Pour Over profile %s", profile["id"])
        return {"profile": copy.deepcopy(profile), "change_id": change_id}

    @classmethod
    def delete_profile(cls, profile_id: str, change_id: str | None = None):
        cls._require_available()
        profile = cls._known_profiles.get(profile_id)
        if profile is None:
            return None
        path = POUR_OVER_PROFILE_PATH.joinpath(f"{profile_id}.json")
        try:
            path.unlink()
        except FileNotFoundError:
            pass
        except OSError as error:
            raise PourOverProfileStorageError("Could not delete profile") from error
        del cls._known_profiles[profile_id]
        try:
            directory_fd = os.open(POUR_OVER_PROFILE_PATH, os.O_RDONLY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
        except OSError as error:
            # The unlink already succeeded and the in-memory catalog must not
            # claim the profile still exists. Log the durability warning but
            # report the logical deletion accurately to the caller.
            logger.warning(
                "Could not fsync Pour Over profile deletion for %s: %s",
                profile_id,
                error.__class__.__name__,
            )
        change_id = change_id or str(uuid.uuid4())
        cls._emit_change("delete", profile_id, change_id)
        logger.info("Deleted Pour Over profile %s", profile_id)
        return {"profile": copy.deepcopy(profile), "change_id": change_id}

    @staticmethod
    def _emit_change(change: str, profile_id: str, change_id: str):
        try:
            from profiles import PROFILE_EVENT, ProfileManager

            ProfileManager._emit_profile_event(
                PROFILE_EVENT(change),
                profile_id,
                change_id,
                brew_type="pour_over",
            )
        except Exception as error:
            logger.warning(
                "Could not emit Pour Over profile change for %s: %s",
                profile_id,
                error.__class__.__name__,
            )

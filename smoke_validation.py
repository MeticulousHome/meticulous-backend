import json
import os
import shutil
import subprocess
import threading
from datetime import datetime, timezone
from pathlib import Path

from config import CONFIG_PATH
from esp_serial.data import MachineStatus
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

DEFAULT_STATE_FILE = Path(CONFIG_PATH).joinpath("smoke-validation.json")
STATE_FILE = Path(os.getenv("SMOKE_VALIDATION_STATE_FILE", DEFAULT_STATE_FILE))
VERSION_FILE = Path(os.getenv("SMOKE_VALIDATION_VERSION_FILE", "/opt/image-build-version"))
PUBLISH_SERVICE = "meticulous-smoke-report.service"
PUBLISH_COMMAND = "/usr/local/bin/meticulous-smoke-report"


class SmokeValidationManager:
    _lock = threading.Lock()
    _shot_sequence = {
        "active": False,
        "saw_heating": False,
        "saw_extraction": False,
        "saw_retracting": False,
        "saw_purge": False,
    }

    @staticmethod
    def _now():
        return datetime.now(timezone.utc).isoformat()

    @staticmethod
    def _current_version():
        try:
            version = VERSION_FILE.read_text(encoding="utf-8").strip()
            return version or "unknown"
        except FileNotFoundError:
            return "unknown"
        except Exception as exc:
            logger.warning(f"Failed to read image version for smoke validation: {exc}")
            return "unknown"

    @classmethod
    def _default_state(cls):
        version = cls._current_version()
        return {
            "image_version": version,
            "post_update_shot_completed": False,
            "post_update_shot_completed_at": None,
            "post_update_shot_completed_version": None,
        }

    @classmethod
    def _read_state(cls):
        try:
            state = json.loads(STATE_FILE.read_text(encoding="utf-8"))
        except FileNotFoundError:
            state = cls._default_state()
        except Exception as exc:
            logger.warning(f"Failed to read smoke validation state: {exc}")
            state = cls._default_state()

        current_version = cls._current_version()
        if state.get("image_version") != current_version:
            state = {
                **state,
                "image_version": current_version,
                "post_update_shot_completed": False,
                "post_update_shot_completed_at": None,
                "post_update_shot_completed_version": None,
            }
            cls._write_state(state)
        return state

    @staticmethod
    def _write_state(state):
        STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
        temp_path = STATE_FILE.with_suffix(".tmp")
        temp_path.write_text(
            json.dumps(state, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        temp_path.replace(STATE_FILE)

    @classmethod
    def get_state(cls):
        with cls._lock:
            return cls._read_state()

    @classmethod
    def get_status(cls, machine):
        state = cls.get_state()
        machine_status = getattr(machine, "data_sensors", None)
        esp_info = getattr(machine, "esp_info", None)

        return {
            "backend_initialized": True,
            "esp32_responding": bool(getattr(machine, "infoReady", False) and esp_info),
            "machine_state": getattr(machine_status, "state", None),
            "machine_status": getattr(machine_status, "status", None),
            "firmware_version": getattr(esp_info, "firmwareV", None),
            "image_version": state["image_version"],
            "post_update_shot_completed": state["post_update_shot_completed"],
            "post_update_shot_completed_at": state["post_update_shot_completed_at"],
            "post_update_shot_completed_version": state["post_update_shot_completed_version"],
        }

    @classmethod
    def observe_status(cls, status, extracting):
        if status == MachineStatus.HEATING:
            cls._shot_sequence = {
                "active": True,
                "saw_heating": True,
                "saw_extraction": False,
                "saw_retracting": False,
                "saw_purge": False,
            }
            return

        if not cls._shot_sequence["active"]:
            return

        is_extraction_stage = extracting and status not in (
            MachineStatus.HEATING,
            MachineStatus.RETRACTING,
            MachineStatus.PURGE,
        )

        if is_extraction_stage:
            cls._shot_sequence["saw_extraction"] = True

        if status == MachineStatus.RETRACTING and cls._shot_sequence["saw_extraction"]:
            cls._shot_sequence["saw_retracting"] = True

        if status == MachineStatus.PURGE and cls._shot_sequence["saw_retracting"]:
            cls._shot_sequence["saw_purge"] = True

        if status == MachineStatus.IDLE:
            cls._shot_sequence = {key: False for key in cls._shot_sequence}

    @classmethod
    def observe_esp_log(cls, message):
        if message != "coffee profile finished. Heater will be kept on until timeout.":
            return

        sequence = cls._shot_sequence.copy()
        cls._shot_sequence = {key: False for key in cls._shot_sequence}
        if all(
            sequence.get(key)
            for key in ("saw_heating", "saw_extraction", "saw_retracting", "saw_purge")
        ):
            cls.mark_post_update_shot_completed()
        else:
            logger.info(f"Shot smoke validation sequence incomplete: {sequence}")

    @classmethod
    def mark_post_update_shot_completed(cls):
        with cls._lock:
            state = cls._read_state()
            current_version = cls._current_version()
            if (
                state.get("post_update_shot_completed") is True
                and state.get("post_update_shot_completed_version") == current_version
            ):
                return False

            state["image_version"] = current_version
            state["post_update_shot_completed"] = True
            state["post_update_shot_completed_at"] = cls._now()
            state["post_update_shot_completed_version"] = current_version
            cls._write_state(state)

        logger.info(f"Post-update shot smoke validation completed for {current_version}")
        cls.request_publish()
        return True

    @staticmethod
    def request_publish():
        if os.path.exists(PUBLISH_COMMAND):
            try:
                subprocess.Popen(
                    [PUBLISH_COMMAND, "--shot"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
            except Exception as exc:
                logger.warning(
                    f"Failed to request Hawkbit shot smoke validation publish: {exc}"
                )
            return

        if shutil.which("systemctl") is None:
            return
        try:
            subprocess.Popen(
                ["systemctl", "start", PUBLISH_SERVICE],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except Exception as exc:
            logger.warning(f"Failed to request Hawkbit smoke validation publish: {exc}")

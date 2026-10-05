import asyncio
import math
import os
import subprocess
import tempfile
import time
import zipfile

from esp_serial.esp_tool_wrapper import ESPToolWrapper, FikaSupportedESP32
from named_thread import NamedThread
from tornado.web import MissingArgumentError

from .base_handler import BaseHandler, LocalAccessHandler
from .api import API, APIVersion

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

HAWKBIT_UPDATER_SERVICE = "rauc-hawkbit-updater"
HAWKBIT_RESTART_TIMEOUT_SECONDS = 60
# The updater unit allows five starts in 20,000 seconds. Limit manual checks to
# two in that window, preserving three starts for boot and configuration changes.
HAWKBIT_CHECK_COOLDOWN_SECONDS = 10_001
HAWKBIT_CHECK_STATE_PATH = "/run/meticulous-backend-hawkbit-update-check"
_update_check_lock = asyncio.Lock()


def os_update_is_active():
    """Read update state lazily without initializing machine hardware on import."""
    from .machine import OSStatus, UpdateOSStatus

    return UpdateOSStatus.last_status in (OSStatus.DOWNLOADING, OSStatus.INSTALLING)


def machine_is_emulated():
    """Read emulation state lazily to avoid initializing machine hardware on import."""
    from machine import Machine

    return Machine.emulated


def _boot_time():
    return time.monotonic()


def _read_last_restart_attempt():
    try:
        with open(HAWKBIT_CHECK_STATE_PATH, encoding="ascii") as state_file:
            return float(state_file.read())
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as error:
        logger.error(
            "Unable to read Hawkbit update check cooldown state: %s",
            type(error).__name__,
        )
        raise


def _record_restart_attempt(attempted_at):
    state_directory = os.path.dirname(HAWKBIT_CHECK_STATE_PATH)
    if state_directory:
        os.makedirs(state_directory, exist_ok=True)
    temporary_path = f"{HAWKBIT_CHECK_STATE_PATH}.{os.getpid()}.tmp"
    try:
        with open(temporary_path, "w", encoding="ascii") as state_file:
            state_file.write(f"{attempted_at:.9f}")
        os.replace(temporary_path, HAWKBIT_CHECK_STATE_PATH)
    except OSError:
        try:
            os.unlink(temporary_path)
        except OSError:
            pass
        raise


def _restart_hawkbit_updater():
    subprocess.run(
        ["systemctl", "restart", HAWKBIT_UPDATER_SERVICE],
        check=True,
        timeout=HAWKBIT_RESTART_TIMEOUT_SECONDS,
    )


class UpdateFirmwareWithZipHandler(BaseHandler):
    def post(self):
        try:
            chip = self.get_argument("chip", None)
        except MissingArgumentError:
            pass

        if not chip:
            self.set_status(400)
            self.write("Missing 'chip' parameter")
            return

        logger.info(f"Flash request for an {chip}")

        chip = FikaSupportedESP32.fromString(chip)
        if not chip:
            self.set_status(400)
            self.write(
                f"Invalid 'chip' parameter. Allowed (case-insensitive): {[e.name for e in FikaSupportedESP32]}"
            )
            return

        # Ensure there is a file in the request
        if "file" not in self.request.files:
            self.set_status(400)
            self.finish("No file uploaded.")
            return

        error_occured = False

        uploaded_files = self.request.files["file"]
        for upload in uploaded_files:
            filename = upload["filename"]
            if not filename.endswith(".zip"):
                if filename in [
                    "firmware.bin",
                    "partitions.bin",
                    "bootloader.bin",
                    "boot_app0.bin",
                ]:
                    error_occured |= not self.handle_file_upload(chip, upload, filename)
                else:
                    self.set_status(400)
                    self.finish(
                        "Invalid file format. Only ZIP files and certain images are accepted."
                    )
                    return
            else:
                error_occured |= not self.handle_zip_upload(upload, chip)

        if error_occured:
            self.write("failure during upload")
            return

        from machine import Machine

        Machine.refreshAvailableFirmware()

        upgradeThread = NamedThread("FWUpgrade", target=Machine.startUpdate)
        upgradeThread.start()

        self.write("success")

    def handle_zip_upload(self, uploaded_file, chip):
        try:
            # Create a temporary file to store the uploaded ZIP
            temp_file = tempfile.NamedTemporaryFile(delete=False)
            temp_file.write(uploaded_file["body"])
            temp_file.close()

            os.makedirs(ESPToolWrapper.getFirmwarePath(chip), exist_ok=True)

            # Unpack the ZIP
            with zipfile.ZipFile(temp_file.name, "r") as zip_ref:
                zip_ref.extractall(ESPToolWrapper.getFirmwarePath(chip))

            # Clean up the temporary file
            os.unlink(temp_file.name)

            logger.info(f"File unpacked to {ESPToolWrapper.getFirmwarePath(chip)}")
            return True
        except zipfile.BadZipFile:
            self.set_status(400)
            self.write("The uploaded file is not a valid ZIP archive.")
            os.unlink(temp_file.name)

        except Exception as e:
            self.set_status(400)
            self.write(f"An error occurred during zip upload: {str(e)}")
            os.unlink(temp_file.name)

        return False

    def handle_file_upload(self, chip, uploaded_file, filename):
        try:
            target = os.path.join(ESPToolWrapper.getFirmwarePath(chip), filename)
            os.makedirs(ESPToolWrapper.getFirmwarePath(chip), exist_ok=True)

            f = open(target, "wb")
            f.write(uploaded_file["body"])
            f.close()

            # Respond to the client
            logger.info(f"File uploaded to {target}")
            return True
        except Exception as e:
            self.set_status(400)
            self.write(f"An error occurred during file upload: {str(e)}")

        return False


class UpdateCheckHandler(LocalAccessHandler):
    async def post(self):
        if machine_is_emulated():
            self.write({"status": "emulated", "message": "Update check skipped"})
            return

        async with _update_check_lock:
            if os_update_is_active():
                self.set_status(409)
                self.write({"status": "error", "error": "An update is already active"})
                return

            now = _boot_time()
            try:
                last_attempt = _read_last_restart_attempt()
                if last_attempt is not None:
                    elapsed = now - last_attempt
                    if 0 <= elapsed < HAWKBIT_CHECK_COOLDOWN_SECONDS:
                        retry_after = math.ceil(HAWKBIT_CHECK_COOLDOWN_SECONDS - elapsed)
                        self.set_status(429)
                        self.set_header("Retry-After", str(retry_after))
                        self.write(
                            {
                                "status": "cooldown",
                                "error": "An update check was recently requested",
                                "retry_after_seconds": retry_after,
                            }
                        )
                        return
                # Record before invoking systemctl: even failed starts consume the
                # unit's start-limit budget.
                _record_restart_attempt(now)
            except (OSError, ValueError):
                self.set_status(500)
                self.write({"status": "error", "error": "Failed to request update check"})
                return

            try:
                await asyncio.to_thread(_restart_hawkbit_updater)
            except subprocess.CalledProcessError as error:
                logger.error(
                    "Hawkbit update check service restart failed with exit code %s",
                    error.returncode,
                )
                self.set_status(500)
                self.write({"status": "error", "error": "Failed to request update check"})
                return
            except subprocess.TimeoutExpired:
                logger.error("Hawkbit update check service restart timed out")
                self.set_status(500)
                self.write({"status": "error", "error": "Failed to request update check"})
                return
            except OSError as error:
                logger.error(
                    "Hawkbit update check service restart failed: %s (errno=%s)",
                    type(error).__name__,
                    error.errno,
                )
                self.set_status(500)
                self.write({"status": "error", "error": "Failed to request update check"})
                return

            self.write({"status": "success"})


API.register_handler(APIVersion.V1, r"/update/firmware", UpdateFirmwareWithZipHandler)
API.register_handler(APIVersion.V1, r"/update/check", UpdateCheckHandler)

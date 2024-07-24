from esp_serial.esp_tool_wrapper import ESPToolWrapper, FikaSupportedESP32
from machine import Machine
import os
import tempfile
import zipfile
import threading

from tornado.web import MissingArgumentError

from .base_handler import BaseHandler
from .api import API, APIVersion

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


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
                f"Invalid 'chip' parameter. Allowed (case-insensitive): {[e.name for e in FikaSupportedESP32]}")
            return

        # Ensure there is a file in the request
        if 'file' not in self.request.files:
            self.set_status(400)
            self.finish("No file uploaded.")
            return

        error_occured = False

        uploaded_files = self.request.files['file']
        for upload in uploaded_files:
            filename = upload['filename']
            if not filename.endswith('.zip'):
                if filename in ["firmware.bin", "partitions.bin", "bootloader.bin", "boot_app0.bin"]:
                    error_occured |= not self.handle_file_upload(chip,
                                                                 upload,
                                                                 filename)
                else:
                    self.set_status(400)
                    self.finish(
                        "Invalid file format. Only ZIP files and certain images are accepted.")
                    return
            else:
                error_occured |= not self.handle_zip_upload(upload, chip)

        if error_occured:
            self.write("failure during upload")
            return

        upgradeThread = threading.Thread(target=Machine.startUpdate)
        upgradeThread.start()

        self.write("success")

    def handle_zip_upload(self, uploaded_file, chip):
        try:
            # Create a temporary file to store the uploaded ZIP
            temp_file = tempfile.NamedTemporaryFile(delete=False)
            temp_file.write(uploaded_file['body'])
            temp_file.close()

            os.makedirs(ESPToolWrapper.getFirmwarePath(chip), exist_ok=True)

            # Unpack the ZIP
            with zipfile.ZipFile(temp_file.name, 'r') as zip_ref:
                zip_ref.extractall(ESPToolWrapper.getFirmwarePath(chip))

            # Clean up the temporary file
            os.unlink(temp_file.name)

            logger.info(
                f"File unpacked to {ESPToolWrapper.getFirmwarePath(chip)}")
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
            target = os.path.join(
                ESPToolWrapper.getFirmwarePath(chip), filename)
            os.makedirs(ESPToolWrapper.getFirmwarePath(chip), exist_ok=True)

            f = open(target, "wb")
            f.write(uploaded_file['body'])
            f.close()

            # Respond to the client
            logger.info(f"File uploaded to {target}")
            return True
        except Exception as e:
            self.set_status(400)
            self.write(f"An error occurred during file upload: {str(e)}")

        return False


API.register_handler(APIVersion.V1, r"/update/firmware",
                     UpdateFirmwareWithZipHandler)

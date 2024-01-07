from esp_serial.esp_tool_wrapper import UPDATE_PATH
from machine import Machine
import os
import tempfile
import zipfile


from .base_handler import BaseHandler
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class UpdateFirmwareWithZipHandler(BaseHandler):
    def post(self):
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
                if filename in ["firmware.bin", "partitions.bin"]:
                    error_occured |= not self.handle_file_upload(
                        upload, filename)
                else:
                    self.set_status(400)
                    self.finish(
                        "Invalid file format. Only ZIP files and certain images are accepted.")
                    return
            else:
                error_occured |= not self.handle_zip_upload(upload)

        if not error_occured:
            self.write("success")
            Machine.startUpdate()

    def handle_zip_upload(self, uploaded_file):
        try:
            # Create a temporary file to store the uploaded ZIP
            temp_file = tempfile.NamedTemporaryFile(delete=False)
            temp_file.write(uploaded_file['body'])
            temp_file.close()

            os.makedirs(UPDATE_PATH, exist_ok=True)

            # Unpack the ZIP
            with zipfile.ZipFile(temp_file.name, 'r') as zip_ref:
                zip_ref.extractall(UPDATE_PATH)

            # Clean up the temporary file
            os.unlink(temp_file.name)

            logger.info(f"File unpacked to {UPDATE_PATH}")
            return True
        except zipfile.BadZipFile:
            self.set_status(400)
            self.write("The uploaded file is not a valid ZIP archive.")
            os.unlink(temp_file.name)

        except Exception as e:
            self.set_status(500)
            self.write(f"An error occurred: {str(e)}")
            os.unlink(temp_file.name)

        return False

    def handle_file_upload(self, uploaded_file, filename):
        try:
            target = os.path.join(UPDATE_PATH, filename)
            os.makedirs(UPDATE_PATH, exist_ok=True)

            f = open(target, "wb")
            f.write(uploaded_file['body'])
            f.close()

            # Respond to the client
            logger.info(f"File uploaded to {target}")
            return True
        except Exception as e:
            self.set_status(500)
            self.write(f"An error occurred: {str(e)}")

        return False


UPDATE_HANDLER = [
    (r"/update/firmware", UpdateFirmwareWithZipHandler),
]

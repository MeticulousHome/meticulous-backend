import os
import tempfile
import zipfile

from .base_handler import BaseHandler
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

from esp_serial.esp_tool_wrapper import UPDATE_PATH
class UpdateFirmwareWithZipHandler(BaseHandler):
    def post(self):
        # Ensure there is a file in the request
        if 'file' not in self.request.files:
            self.set_status(400)
            self.finish("No file uploaded.")
            return

        uploaded_file = self.request.files['file'][0]
        filename = uploaded_file['filename']

        # Validate ZIP format based on file extension
        if not filename.endswith('.zip'):
            self.set_status(400)
            self.finish("Invalid file format. Only ZIP files are accepted.")
            return

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

            # Respond to the client
            self.write(f"File unpacked to {UPDATE_PATH}")

            logger.warning("FIXME: add flashing logic here")

        except zipfile.BadZipFile:
            self.set_status(400)
            self.finish("The uploaded file is not a valid ZIP archive.")
            os.unlink(temp_file.name)
        except Exception as e:
            self.set_status(500)
            self.finish(f"An error occurred: {str(e)}")
            os.unlink(temp_file.name)


UPDATE_HANDLER = [
        (r"/update/firmware", UpdateFirmwareWithZipHandler),
    ]

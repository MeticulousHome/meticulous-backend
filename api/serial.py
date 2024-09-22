from log import MeticulousLogger
from machine import Machine

from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)


WEBPAGE = """
<!DOCTYPE html>
<html>
<head>
    <title>Product Form</title>
</head>
<body>
    <form action="serial" method="post">
        <label for="color">Color:</label>
        <input type="text" id="color" name="color" value="black"><br><br>

        <label for="serial">Serial:</label>
        <input type="text" id="serial" name="serial"><br><br>

        <label for="batch_number">Batch Number:</label>
        <input type="text" id="batch_number" name="batch_number"><br><br>

        <label for="build_date">Build Date:</label>
        <input type="date" id="build_date" name="build_date"><br><br>

        <input type="submit" value="Submit">
    </form>

    <script>
        // Set the build date to today's date
        document.addEventListener('DOMContentLoaded', () => {
            const today = new Date().toISOString().split('T')[0];
            document.getElementById('build_date').value = today;
        });
    </script>
</body>
</html>
"""


class SerialNumberHandler(BaseHandler):
    def get(self):
        self.set_default_headers()
        self.set_header("Content-Type", "text/html")
        self.write(WEBPAGE)
        return

    def post(self):
        self.set_header("Content-Type", "text/html")

        color = self.get_body_argument("color", default="black")
        serial = self.get_body_argument("serial")
        batch_number = self.get_body_argument("batch_number")
        build_date = self.get_body_argument("build_date")

        if color == "":
            self.write("Color is required")
            return

        if serial == "":
            self.write("Serial is required")
            return

        if batch_number == "":
            self.write("Batch number is required")
            return

        if build_date == "":
            self.write("Build date is required")
            return

        Machine.setSerial(color, serial, batch_number, build_date)
        self.write(
            f"Received Data:<br>Color: {color}<br>Serial: {serial}<br>"
            f"Batch Number: {batch_number}<br>Build Date: {build_date}"
        )


API.register_handler(APIVersion.V1, r"/serial", SerialNumberHandler),

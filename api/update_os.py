from .base_handler import BaseHandler
from update_os import RaucManager  # Assuming update_os.py is accessible
from .api import API, APIVersion

class RAUCStatusHandler(BaseHandler):
    def get(self):
        rauc_info = RaucManager.get_rauc_status()
        self.write(rauc_info.output)

class StartOSUpdateHandler(BaseHandler):
    def get(self):
        RaucManager.start_os_update()  # Run the command in a separate thread
        self.write("Update process started. Check show_update_messages for progress.")

class ShowUpdateMessagesHandler(BaseHandler):
    def get(self):
        update_info = RaucManager.get_update_messages()
        self.write(update_info.output)

# Register the routes when importing the module
API.register_handler(APIVersion.V1, r"/update_os/rauc_status", RAUCStatusHandler)
API.register_handler(APIVersion.V1, r"/update_os/start_os_update", StartOSUpdateHandler)
API.register_handler(APIVersion.V1, r"/update_os/show_update_messages", ShowUpdateMessagesHandler)


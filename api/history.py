import json
import os

import tornado
import tornado.web
import zstandard as zstd
from pydantic import ValidationError

from log import MeticulousLogger
from shot_database import SearchParams, ShotDataBase
from shot_debug_manager import DEBUG_HISTORY_PATH
from shot_manager import ShotManager, HISTORY_PATH

from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)
last_version_path = f"/api/{APIVersion.latest_version().name.lower()}"


class ZstdHistoryHandler(tornado.web.StaticFileHandler):
    async def get(self, path):
        self.set_header("Content-Type", "application/json")

        # Check if the path is a directory
        full_path = self.get_absolute_path(self.root, path)
        # Needed for caching in the tornado.web.StaticFileHandler class
        self.absolute_path = full_path

        compressed_path = f"{full_path}.zst"
        if os.path.isdir(full_path):
            # If it's a directory, show the JSON listing
            return await self.list_directory(full_path)
        elif not os.path.exists(full_path) or not os.path.isfile(full_path):
            logger.info(
                f"File doesn't exist: {full_path}, checking if it exists compressed"
            )

            if os.path.exists(compressed_path) and os.path.isfile(compressed_path):
                logger.info("File exists compressed instead")
                self.absolute_path = compressed_path
                return await self.serve_zstd_file(compressed_path)

            # If the path doesn't exist or isn't a file, raise a 404 error
            self.set_status(404)
            self.write(
                {"status": "error", "error": "history entry not found", "path": path}
            )
            return
        elif full_path.endswith(".zst"):
            logger.info("dealing")
            # Handle .zstd compressed file
            return await self.serve_zstd_file(full_path)
        else:
            logger.info("File exists on disk")
            # Fallback to default behavior for regular files
            response = await super().get(path, True)
            self.set_header("Content-Type", "application/json")
            return response

    async def serve_zstd_file(self, full_path):
        logger.info(f"Serving File: {full_path}")
        with open(full_path, "rb") as compressed_file:
            decompressor = zstd.ZstdDecompressor()
            decompressed_content = decompressor.stream_reader(compressed_file)
            self.set_header("Content-Type", "application/json")
            self.write(decompressed_content.read())
            self.finish()

    async def list_directory(self, full_path):

        if not os.path.isdir(full_path):
            raise tornado.web.HTTPError(404)

        # List directory contents and sort them
        files = sorted(os.listdir(full_path))
        files_info = []
        for f in files:
            file_path = f
            files_info.append({"name": f.rstrip(".zst"), "url": file_path})

        self.set_header("Content-Type", "application/json")
        self.write(json.dumps(files_info))
        self.finish()


class ProfileSearchHandler(BaseHandler):
    def get(self):
        query = self.get_argument("query", "")
        # Logic to fetch profiles based on name_prefix
        profiles = ShotDataBase.autocomplete_profile_name(query)
        self.write({"profiles": profiles})


class CurrentShotHandler(BaseHandler):
    def get(self):
        current = ShotManager.getCurrentShot()
        self.write(json.dumps(current))


class LastShotHandler(BaseHandler):
    def get(self):
        last = ShotManager.getLastShot()
        self.write(json.dumps(last))


class HistoryHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body)
            params = SearchParams(**data)
        except json.JSONDecodeError:
            self.set_status(400)
            self.write({"error": "Invalid JSON"})
            return
        except ValidationError as e:
            self.set_status(422)
            self.write(e.json())
            return

        results = ShotDataBase.search_history(params)
        self.write({"history": results})

    def get(self):
        # get all entries
        params = SearchParams(dump_data=False, max_results=-1)

        results = ShotDataBase.search_history(params)
        self.write({"history": results})


API.register_handler(APIVersion.V1, r"/history/search", ProfileSearchHandler),
API.register_handler(APIVersion.V1, r"/history/current", CurrentShotHandler),
API.register_handler(APIVersion.V1, r"/history/last", LastShotHandler),

API.register_handler(APIVersion.V1, r"/history", HistoryHandler),

API.register_handler(
    APIVersion.V1,
    r"/history/debug",
    tornado.web.RedirectHandler,
    url=f"{last_version_path}/history/debug/",
),

API.register_handler(
    APIVersion.V1, r"/history/debug/(.*)", ZstdHistoryHandler, path=DEBUG_HISTORY_PATH
),

API.register_handler(
    APIVersion.V1,
    r"/history/files",
    tornado.web.RedirectHandler,
    url=f"{last_version_path}/history/files/",
),

API.register_handler(
    APIVersion.V1, r"/history/files/(.*)", ZstdHistoryHandler, path=HISTORY_PATH
),

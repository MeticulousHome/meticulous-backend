import os
import json

import tornado
import tornado.web
import zstandard as zstd
from log import MeticulousLogger
from shot_debug_manager import DEBUG_HISTORY_PATH
from shot_manager import HISTORY_PATH

from config import *

from .base_handler import BaseHandler
from .api import API, APIVersion

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
            logger.info(f"File doesn't exist: {full_path}, checking if it exists compressed")

            if os.path.exists(compressed_path) and os.path.isfile(compressed_path):
                logger.info("File exists compressed instead")
                self.absolute_path = compressed_path
                return await self.serve_zstd_file(compressed_path)

            # If the path doesn't exist or isn't a file, raise a 404 error
            self.set_status(404)
            self.write(
                {"status": "error", "error": "history entry not found", "path": path})
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
        with open(full_path, 'rb') as compressed_file:
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
            files_info.append({"name": f.rstrip('.zst'), "url": file_path})

        self.set_header("Content-Type", "application/json")
        self.write(json.dumps(files_info))
        self.finish()


API.register_handler(APIVersion.V1, r'/history/debug',
                     tornado.web.RedirectHandler, url=f"{last_version_path}/history/debug/"),

API.register_handler(APIVersion.V1, r'/history/debug/(.*)',
                     ZstdHistoryHandler, path=DEBUG_HISTORY_PATH),

API.register_handler(APIVersion.V1, r'/history',
                     tornado.web.RedirectHandler, url=f"{last_version_path}/history/"),

API.register_handler(APIVersion.V1, r'/history/(.*)',
                     ZstdHistoryHandler, path=HISTORY_PATH),

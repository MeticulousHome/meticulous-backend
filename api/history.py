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
    async def get(self, path, include_body=True):
        # Check if the path is a directory
        full_path = self.get_absolute_path(self.root, path)
        if os.path.isdir(full_path):
            # If it's a directory, show the JSON listing
            return await self.list_directory(path)
        elif not os.path.exists(full_path) or not os.path.isfile(full_path):
            logger.info(f"File doesn't exist: {full_path}")
            # If the path doesn't exist or isn't a file, raise a 404 error
            raise tornado.web.HTTPError(404)
        elif full_path.endswith(".zst"):
            self.absolute_path = full_path
            # Handle .zstd compressed file
            return await self.serve_zstd_file(full_path, include_body)
        else:
            # Fallback to default behavior for regular files
            return await super().get(path, include_body)

    async def serve_zstd_file(self, full_path, include_body):
        logger.info(f"Serving File: {include_body}")
        with open(full_path, 'rb') as compressed_file:
            decompressor = zstd.ZstdDecompressor()
            decompressed_content = decompressor.stream_reader(compressed_file)
            if include_body:
                self.set_header("Content-Type", self.get_content_type())
                self.write(decompressed_content.read())
            self.finish()

    async def list_directory(self, path):
        full_path = self.get_absolute_path(self.root, path)
        self.absolute_path = full_path
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

    def get_content_type(self):
        # Ensure content type for served files is correct
        return 'application/octet-stream'

API.register_handler(APIVersion.V1, r'/history/debug',
                     tornado.web.RedirectHandler, url=f"{last_version_path}/history/debug/"),

API.register_handler(APIVersion.V1, r'/history/debug/(.*)',
                     ZstdHistoryHandler, path=DEBUG_HISTORY_PATH),

API.register_handler(APIVersion.V1, r'/history',
                     tornado.web.RedirectHandler, url=f"{last_version_path}/history/"),

API.register_handler(APIVersion.V1, r'/history/((?!debug)).*)',
                     ZstdHistoryHandler, path=HISTORY_PATH),

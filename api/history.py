import os

import pyqrcode
import tornado
import zstandard as zstd
from ble_gatt import PORT, GATTServer
from log import MeticulousLogger
from wifi import WifiManager

from config import *

from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)

from shot_debug_manager import DEBUG_HISTORY_PATH
from shot_manager import HISTORY_PATH


import tornado.web
import zstandard as zstd
import os
import mimetypes

class ZstdHistoryHandler(tornado.web.StaticFileHandler):
    # FIXME this breaks tornados caching. However writing our own versions sounds much more dangerous
    # so we accept it for now and add it to our benchmarking
    async def get(self, path, include_body=True):
        # Check if
        # the path is a directory
        full_path = self.get_absolute_path(self.root, path)
        if os.path.isdir(full_path):
            # If it's a directory, show the listing
            return await self.list_directory(path)
        elif not os.path.exists(full_path) or not os.path.isfile(full_path):
            logger.info(f"File doesn't exist: {full_path}" )
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
        
        files = os.listdir(full_path)
        self.set_header("Content-Type", "text/html")
        self.write("<html><body><ul>")
        for f in files:
            if path != "":
                ref = f"{path}/{f}"
            else:
                ref = f"{f}"
            if f.endswith(".zst"):
                f = f[:-4]
            self.write(f'<li><a href="{ref}">{f}</a></li>')
        self.write("</ul></body></html>")
        self.finish()
    
    def get_content_type(self):
        return 'application/json'

class ZstdDebugHandler(ZstdHistoryHandler):
    def get_content_type(self):
        return "text/plain; charset=utf-8"

HISTORY_HANDLER = [
        (r'/history/debug', tornado.web.RedirectHandler, {"url":"/history/debug/"}),
        (r'/history/debug/(.*)', ZstdDebugHandler, {"path": DEBUG_HISTORY_PATH}),
        (r'/history', tornado.web.RedirectHandler, {"url":"/history/"}),
        (r'/history/(.*)', ZstdHistoryHandler, {"path": HISTORY_PATH}),
    ]

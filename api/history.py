import json
import os

import tornado
import tornado.web
import zstandard as zstd
from pydantic import ValidationError
from typing import Optional

from log import MeticulousLogger
from shot_database import SearchParams, ShotDataBase, SearchOrder, SearchOrderBy
from config import DEBUG_HISTORY_PATH, SHOT_PATH
from shot_manager import ShotManager

from .api import API, APIVersion
from .base_handler import BaseHandler
import asyncio
from shot_debug_manager import ShotDebugManager
from pathlib import Path
from config import MeticulousConfig, CONFIG_SYSTEM, DEVICE_IDENTIFIER

logger = MeticulousLogger.getLogger(__name__)
last_version_path = f"/api/{APIVersion.latest_version().name.lower()}"


class LastDebugFileHandler(BaseHandler):
    """Handler for retrieving the most recent debug file."""

    DEBUG_FILE_EXTENSION = ".zst"

    async def get(self):
        """Handle GET request for the latest debug file."""
        try:
            latest_debug_file = self._find_latest_debug_file()
            if not latest_debug_file:
                raise FileNotFoundError("No debug files found")

            # Convert absolute path to relative path for redirect
            relative_path = latest_debug_file.relative_to(DEBUG_HISTORY_PATH)
            self.redirect(f"{last_version_path}/history/debug/{relative_path}")

        except FileNotFoundError as e:
            self._handle_error(404, str(e))
        except Exception as e:
            logger.error(f"Unexpected error getting last debug file: {e}")
            self._handle_error(500, "Internal server error")

    def _find_latest_debug_file(self):
        """Find the most recently modified debug file across all directories."""
        debug_path = Path(DEBUG_HISTORY_PATH)

        if not debug_path.exists():
            return None

        # Find all debug files recursively
        pattern = f"**/*{self.DEBUG_FILE_EXTENSION}"
        debug_files = list(debug_path.glob(pattern))

        if not debug_files:
            return None

        # Return the file with the most recent modification time
        return max(debug_files, key=lambda f: f.stat().st_mtime)

    def _handle_error(self, status_code, message):
        """Handle error responses consistently."""
        self.set_status(status_code)
        self.write({"status": "error", "error": message})


class CompressedDebugHistoryHandler(BaseHandler):
    async def get(self):
        async def compress_debug_file():
            loop = asyncio.get_event_loop()
            # Run the compression in a separate thread to avoid blocking the event loop
            zip_name = await loop.run_in_executor(
                None, ShotDebugManager.zipAllDebugShots
            )
            return zip_name

        try:
            result = await compress_debug_file()
            self.redirect(f"{last_version_path}/history/debug/{result}")
        except Exception as e:
            logger.error(f"Error compressing debug file: {e}")
            self.set_status(500)
            self.write({"status": "error", "error": "Internal server error"})


class ZstdHistoryHandler(tornado.web.StaticFileHandler):
    def set_default_headers(self):
        BaseHandler.set_default_headers(self)

    def compute_etag(self) -> Optional[str]:
        """Override to disable ETag generation on directories"""
        if os.path.isdir(self.absolute_path):
            return None
        if not os.path.exists(self.absolute_path):
            return None

        return super().compute_etag()

    async def get(self, path):
        self.set_header("Content-Type", "application/json")
        serve_compressed = self.get_query_argument("compressed", None) is not None

        # Check if the path is a directory
        full_path = self.get_absolute_path(self.root, path)
        # Needed for caching in the tornado.web.StaticFileHandler class
        self.absolute_path = full_path

        compressed_path = f"{full_path}.zst"
        if os.path.isdir(full_path):
            logger.info(f"Request for a Directory:{full_path}")
            # If it's a directory, show the JSON listing
            await self.list_directory(full_path)
            return
        elif not os.path.exists(full_path) or not os.path.isfile(full_path):
            logger.info(
                f"File doesn't exist: {full_path}, checking if it exists compressed"
            )

            if os.path.exists(compressed_path) and os.path.isfile(compressed_path):
                logger.info("File exists compressed instead")
                self.absolute_path = compressed_path
                return await self.serve_decompressed_file(compressed_path)

            # If the path doesn't exist or isn't a file, raise a 404 error
            self.set_status(404)
            self.write(
                {"status": "error", "error": "history entry not found", "path": path}
            )
            return
        elif full_path.endswith(".zst") and not serve_compressed:
            logger.info("dealing")
            # Handle .zstd compressed file
            return await self.serve_decompressed_file(full_path)
        else:
            logger.info("File exists on disk")
            # Fallback to default behavior for regular files
            if full_path.endswith(".zst"):
                self.set_header("Content-Type", "application/octet-stream")
                device_name = "".join(
                    MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER]
                )
                file_date_formatted = path.split(os.path.sep)[0].replace("-", "_")
                served_file_name = (
                    f"{device_name}_{file_date_formatted}_{os.path.basename(path)}"
                )
                self.set_header(
                    "Content-Disposition",
                    f'attachment; filename="{served_file_name}"',
                )
            else:
                self.set_header("Content-Type", "application/json")
            await super().get(path, True)
            self.finish()

    async def serve_decompressed_file(self, full_path):
        logger.info(f"Serving File: {full_path}")
        with open(full_path, "rb") as compressed_file:
            decompressor = zstd.ZstdDecompressor()
            decompressed_content = decompressor.stream_reader(compressed_file)
            logger.warning(full_path)
            raw = decompressed_content.read()
            if b": Infinity" in raw:
                logger.warning(f"Patching Infinity token in shot file: {full_path}")
                raw = raw.replace(b": Infinity", b": 0.0")
            if full_path.endswith(".csv.zst"):
                self.set_header("Content-Type", "text/csv")
            else:
                self.set_header("Content-Type", "application/json")
            self.write(raw)
            self.finish()

    async def list_directory(self, full_path):

        if not os.path.isdir(full_path):
            raise tornado.web.HTTPError(404)

        # List directory contents and sort them
        files = sorted(
            os.listdir(full_path),
            key=lambda f: os.path.getmtime(os.path.join(full_path, f)),
            reverse=True,
        )
        logger.info("Files sorted")
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
    async def get(self):
        async def fetch_last_shot():
            loop = asyncio.get_event_loop()
            last = await loop.run_in_executor(None, ShotManager.getLastShot)
            return json.dumps(last)

        last_shot_json = await fetch_last_shot()
        self.write(last_shot_json)


class HistoryHandler(BaseHandler):
    async def searchHistory(self, params: SearchParams):
        loop = asyncio.get_event_loop()
        last = await loop.run_in_executor(None, ShotDataBase.search_history, params)
        return last

    async def post(self):
        try:
            data = json.loads(self.request.body)
            params = SearchParams(**data)
        except json.JSONDecodeError as e:
            self.set_status(400)
            self.write({"error": "Invalid JSON", "details": str(e)})
            return
        except ValidationError as e:
            self.set_status(422)
            self.write(e.json())
            return

        results = await self.searchHistory(params)
        self.write({"history": results})

    async def get(self):
        # get all entries
        params = SearchParams(
            query=self.get_query_argument("query", None),
            ids=self.get_query_arguments("ids"),
            start_date=self.get_query_argument("start_date", None),
            end_date=self.get_query_argument("end_date", None),
            order_by=self.get_query_arguments("order_by", [SearchOrderBy.date]),
            sort=self.get_query_argument("sort", SearchOrder.descending),
            max_results=self.get_query_argument("max_results", 20),
            dump_data=self.get_query_argument("dump_data", True),
        )

        results = await self.searchHistory(params)
        self.write({"history": results})


class StatisticsHandler(BaseHandler):
    def get(self):
        results = ShotDataBase.statistics()
        self.write(results)


class ShotRatingHandler(BaseHandler):
    def get(self, shot_id):
        try:
            rating = ShotDataBase.get_shot_rating(shot_id)
            self.write({"shot_id": shot_id, "rating": rating})
        except Exception as e:
            logger.error(f"Error getting shot rating: {e}")
            self.set_status(500)
            self.write({"status": "error", "error": "Internal server error"})

    def post(self, shot_id):
        if not shot_id or shot_id == "":
            self.set_status(400)
            self.write({"status": "error", "error": "Invalid shot ID"})
            return

        try:
            data = json.loads(self.request.body)
            rating = data.get("rating", None)

            if rating not in ["like", "dislike", None]:
                self.set_status(400)
                self.write(
                    {
                        "status": "error",
                        "error": "Invalid rating value. Use 'like', 'dislike', or null",
                    }
                )
                return

            success = ShotDataBase.rate_shot(shot_id, rating)

            if success:
                self.write({"status": "ok", "shot_id": shot_id, "rating": rating})
            else:
                self.set_status(404)
                self.write(
                    {"status": "error", "error": "Shot not found or rating failed"}
                )
        except json.JSONDecodeError:
            self.set_status(400)
            self.write({"status": "error", "error": "Invalid JSON"})
        except Exception as e:
            logger.error(f"Error updating shot rating: {e}")
            self.set_status(500)
            self.write({"status": "error", "error": "Internal server error"})


API.register_handler(APIVersion.V1, r"/history/search", ProfileSearchHandler),
API.register_handler(APIVersion.V1, r"/history/current", CurrentShotHandler),
API.register_handler(APIVersion.V1, r"/history/last", LastShotHandler),
API.register_handler(APIVersion.V1, r"/history/stats", StatisticsHandler),

API.register_handler(APIVersion.V1, r"/history", HistoryHandler),
API.register_handler(APIVersion.V1, r"/history/last-debug-file", LastDebugFileHandler),
API.register_handler(APIVersion.V1, r"/history/rating/(.*)", ShotRatingHandler),

API.register_handler(
    APIVersion.V1,
    r"/history/debug",
    tornado.web.RedirectHandler,
    url=f"{last_version_path}/history/debug/",
),

API.register_handler(
    APIVersion.V1, r"/history/debug.zip", CompressedDebugHistoryHandler
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
    APIVersion.V1, r"/history/files/(.*)", ZstdHistoryHandler, path=SHOT_PATH
),

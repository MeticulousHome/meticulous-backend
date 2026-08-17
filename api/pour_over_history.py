import json
from pathlib import Path

import tornado.web
import zstandard as zstd
from pydantic import ValidationError

from config import POUR_OVER_PATH
from log import MeticulousLogger
from pour_over_history import (
    PourOverHistoryConflictError,
    PourOverHistoryManager,
    PourOverHistoryRecordTooLargeError,
    PourOverSession,
    MAX_POUR_OVER_RECORD_BYTES,
    read_compressed_record,
)

from .api import API, APIVersion
from .base_handler import BaseHandler, LocalAccessHandler

logger = MeticulousLogger.getLogger(__name__)
last_version_path = f"/api/{APIVersion.latest_version().name.lower()}"
MAX_POUR_OVER_BODY_BYTES = MAX_POUR_OVER_RECORD_BYTES
MAX_DIRECTORY_RESULTS = 200


class PourOverFileHandler(BaseHandler):
    def initialize(self, path):
        self.root = Path(path).resolve()

    async def get(self, relative_path):
        try:
            requested = self.root.joinpath(relative_path).resolve()
            requested.relative_to(self.root)
        except ValueError:
            self.set_status(404)
            self.write({"status": "error", "error": "history entry not found"})
            return

        if requested.is_dir():
            entries = sorted(
                requested.iterdir(), key=lambda entry: entry.stat().st_mtime, reverse=True
            )[:MAX_DIRECTORY_RESULTS]
            self.write(
                [
                    {
                        "name": entry.name.removesuffix(".zst"),
                        "url": entry.name,
                    }
                    for entry in entries
                ]
            )
            return

        compressed_path = requested
        if not compressed_path.is_file() and not str(compressed_path).endswith(".zst"):
            compressed_path = Path(f"{compressed_path}.zst")
        if not compressed_path.is_file():
            self.set_status(404)
            self.write(
                {
                    "status": "error",
                    "error": "history entry not found",
                    "path": relative_path,
                }
            )
            return

        try:
            raw = read_compressed_record(compressed_path)
        except (zstd.ZstdError, PourOverHistoryRecordTooLargeError):
            logger.warning("Invalid compressed Pour Over history file: %s", compressed_path)
            self.set_status(500)
            self.write({"status": "error", "error": "Invalid history entry"})
            return
        self.set_header("Content-Type", "application/json")
        self.write(raw)


class PourOverHistoryHandler(LocalAccessHandler):
    async def post(self):
        if len(self.request.body) > MAX_POUR_OVER_BODY_BYTES:
            self.set_status(413)
            self.write({"status": "error", "error": "Pour Over record is too large"})
            return
        try:
            raw = json.loads(self.request.body)
            session = PourOverSession.model_validate(raw)
        except (json.JSONDecodeError, UnicodeDecodeError):
            self.set_status(400)
            self.write({"status": "error", "error": "Invalid JSON"})
            return
        except ValidationError as error:
            self.set_status(422)
            self.write(
                {
                    "status": "error",
                    "error": "Invalid Pour Over record",
                    "details": json.loads(error.json(include_url=False)),
                }
            )
            return

        try:
            history, created = PourOverHistoryManager.save(session)
        except PourOverHistoryRecordTooLargeError:
            self.set_status(413)
            self.write({"status": "error", "error": "Pour Over record is too large"})
            return
        except PourOverHistoryConflictError:
            self.set_status(409)
            self.write(
                {
                    "status": "error",
                    "error": "A different Pour Over record already uses this id",
                }
            )
            return
        except Exception as error:
            logger.exception("Failed to save Pour Over history", exc_info=error)
            self.set_status(500)
            self.write({"status": "error", "error": "Could not save Pour Over record"})
            return

        self.set_status(201 if created else 200)
        self.write({"status": "created" if created else "existing", "history": history})

    async def get(self):
        try:
            max_results = min(max(int(self.get_query_argument("max_results", "50")), 1), 200)
        except ValueError:
            self.set_status(400)
            self.write({"status": "error", "error": "max_results must be an integer"})
            return
        descending = self.get_query_argument("sort", "desc") != "asc"
        after = self.get_query_argument("after", None)
        mode = self.get_query_argument("mode", None)
        if mode not in (None, "free_pour", "profile"):
            self.set_status(400)
            self.write({"status": "error", "error": "Invalid Pour Over mode"})
            return
        history = PourOverHistoryManager.search(
            max_results=max_results, descending=descending, after=after, mode=mode
        )
        self.write({"history": history})


class LastPourOverHandler(BaseHandler):
    async def get(self):
        history = PourOverHistoryManager.latest()
        if history is None:
            self.set_status(404)
            self.write({"status": "error", "error": "No Pour Over records found"})
            return
        self.write(history)


API.register_handler(APIVersion.V1, r"/history/pour-over", PourOverHistoryHandler)
API.register_handler(APIVersion.V1, r"/history/pour-over/last", LastPourOverHandler)
API.register_handler(
    APIVersion.V1,
    r"/history/pour-over/files",
    tornado.web.RedirectHandler,
    url=f"{last_version_path}/history/pour-over/files/",
)
API.register_handler(
    APIVersion.V1,
    r"/history/pour-over/files/(.*)",
    PourOverFileHandler,
    path=POUR_OVER_PATH,
)

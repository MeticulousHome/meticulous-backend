import json

from log import MeticulousLogger
from shot_database import ShotDataBase
from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)


class ShotRatingHandler(BaseHandler):
    """Handler to manage individual shot ratings"""

    def get(self, history_id: str):
        """Gets the rating of a specific shot"""
        try:
            history_id = int(history_id)
            rating = ShotDataBase.get_shot_rating(history_id)

            if rating is None:
                self.set_status(404)
                self.write({"error": "Shot not found"})
                return

            self.write({"history_id": history_id, "rating": rating})

        except ValueError:
            self.set_status(400)
            self.write({"error": "Invalid history ID"})

    def post(self, history_id: str):
        """Sets or updates the rating of a shot"""
        try:
            history_id = int(history_id)
            data = json.loads(self.request.body)

            if "rating" not in data:
                self.set_status(400)
                self.write({"error": "The 'rating' field is required"})
                return

            rating = data["rating"]
            if rating not in ["good", "bad", "unrated"]:
                self.set_status(400)
                self.write({"error": "Rating must be 'good', 'bad', or 'unrated'"})
                return

            success = ShotDataBase.rate_shot(history_id, rating)

            if not success:
                self.set_status(500)
                self.write({"error": "Error rating the shot"})
                return

            self.write({"success": True, "history_id": history_id, "rating": rating})

        except ValueError:
            self.set_status(400)
            self.write({"error": "Invalid history ID"})
        except json.JSONDecodeError:
            self.set_status(400)
            self.write({"error": "Invalid JSON"})


class ShotRatingListHandler(BaseHandler):
    """Handler to list all ratings"""

    def get(self, *args):
        """Gets a list of all ratings"""
        ratings = ShotDataBase.get_rating_statistics()
        if ratings is None:
            self.set_status(500)
            self.write({"error": "Error retrieving ratings"})
            return
        self.write({"ratings": ratings})


class ShotRatingsStatsHandler(BaseHandler):
    """Handler to get rating statistics"""

    def get(self):
        """Gets general rating statistics"""
        stats = ShotDataBase.get_rating_statistics()

        if stats is None:
            self.set_status(500)
            self.write({"error": "Error retrieving statistics"})
            return

        self.write(stats)


# Register the endpoints in the API
API.register_handler(APIVersion.V1, r"/shots/ratings/stats", ShotRatingsStatsHandler)
API.register_handler(APIVersion.V1, r"/shots/ratings/([0-9]+)", ShotRatingHandler)
API.register_handler(APIVersion.V1, r"/shots/ratings/?", ShotRatingListHandler)

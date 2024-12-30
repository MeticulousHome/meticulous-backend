import json

from log import MeticulousLogger
from shot_database import ShotDataBase
from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)


class ShotRatingHandler(BaseHandler):
    """Handler para gestionar las calificaciones individuales de shots"""

    def get(self, history_id: str):
        """Obtiene la calificación de un shot específico"""
        try:
            history_id = int(history_id)
            rating = ShotDataBase.get_shot_rating(history_id)

            if rating is None:
                self.set_status(404)
                self.write({"error": "Shot no encontrado"})
                return

            self.write({"history_id": history_id, "rating": rating})

        except ValueError:
            self.set_status(400)
            self.write({"error": "ID de historia inválido"})

    def post(self, history_id: str):
        """Establece o actualiza la calificación de un shot"""
        try:
            history_id = int(history_id)
            data = json.loads(self.request.body)

            if "rating" not in data:
                self.set_status(400)
                self.write({"error": "Se requiere el campo 'rating'"})
                return

            rating = data["rating"]
            if rating not in ["good", "bad", "unrated"]:
                self.set_status(400)
                self.write({"error": "Rating debe ser 'good', 'bad' o 'unrated'"})
                return

            success = ShotDataBase.rate_shot(history_id, rating)

            if not success:
                self.set_status(500)
                self.write({"error": "Error al calificar el shot"})
                return

            self.write({"success": True, "history_id": history_id, "rating": rating})

        except ValueError:
            self.set_status(400)
            self.write({"error": "ID de historia inválido"})
        except json.JSONDecodeError:
            self.set_status(400)
            self.write({"error": "JSON inválido"})


class ShotRatingListHandler(BaseHandler):
    """Handler para listar todas las calificaciones"""

    def get(self, *args):  # Modificado para aceptar argumentos opcionales
        """Obtiene lista de todas las calificaciones"""
        ratings = ShotDataBase.get_rating_statistics()
        if ratings is None:
            self.set_status(500)
            self.write({"error": "Error al obtener calificaciones"})
            return
        self.write({"ratings": ratings})


class ShotRatingsStatsHandler(BaseHandler):
    """Handler para obtener estadísticas de calificaciones"""

    def get(self):
        """Obtiene estadísticas generales de calificaciones"""
        stats = ShotDataBase.get_rating_statistics()

        if stats is None:
            self.set_status(500)
            self.write({"error": "Error al obtener estadísticas"})
            return

        self.write(stats)


# Registrar los endpoints en la API
API.register_handler(APIVersion.V1, r"/shots/ratings/stats", ShotRatingsStatsHandler)
API.register_handler(APIVersion.V1, r"/shots/ratings/([0-9]+)", ShotRatingHandler)
API.register_handler(APIVersion.V1, r"/shots/ratings/?", ShotRatingListHandler)

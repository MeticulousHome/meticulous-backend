from enum import Enum, auto
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class APIVersion(Enum):
    V1 = auto()
    # Add more versions when needed
    # V2 = auto()

    @classmethod
    def latest_version(cls):
        # Returns the enum member with the highest value
        return max(cls, key=lambda v: v.value)

    @classmethod
    def from_string(cls, version_str):
        """Converts version string to APIVersion enum."""
        mapping = {
            "v1": cls.V1,
        }
        return mapping.get(version_str.lower(), None)


class API:
    # Initialize _versions with all versions up to the latest
    _versions = {
        version: {}
        for version in APIVersion
        if version.value <= APIVersion.latest_version().value
    }

    @classmethod
    def register_handler(cls, r_version: APIVersion, r_path: str, r_handler, **kwargs):
        if r_version not in cls._versions:
            cls._versions[r_version] = {}
        cls._versions[r_version][r_path] = (r_handler, kwargs)

    @classmethod
    def get_routes(cls):
        from . import action as _action  # noqa
        from . import history as _history  # noqa
        from . import notifications as _noti  # noqa
        from . import profiles as _profiles  # noqa
        from . import settings as _settings  # noqa
        from . import update as _update  # noqa
        from . import wifi as _wifi  # noqa
        from . import sounds as _sounds  # noqa
        from . import machine as _machine  # noqa

        routes = []
        logger.info("API Routes registered:")
        for version, paths in cls._versions.items():
            version_path = f"/api/{version.name.lower()}"
            logger.info(f"  {version_path}")
            # Create a temporary array so we can sort the routes for printing
            version_routes = []
            for path, (handler, kwargs) in paths.items():
                route_path = f"{version_path}{path}"
                version_routes.append((route_path, handler, kwargs))
            version_routes.sort(key=lambda route: route[0])
            for path, _, _ in version_routes:
                logger.info(f"    {path}")

            routes.extend(version_routes)
        return routes

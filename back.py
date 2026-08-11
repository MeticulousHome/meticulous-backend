import sentry_sdk
from sentry_sdk.integrations.asyncio import AsyncioIntegration

import os

from sentry_privacy import drop_breadcrumb, sanitize_sentry_event

BACKEND = os.getenv("BACKEND", "FIKA").upper()
SENTRY = os.getenv("SENTRY", "False").lower() in ("true", "1", "y")
SENTRY_ENABLED = BACKEND == "FIKA" or SENTRY


def is_tornado_session_disconnected(event):
    exc = event.get("exception", {})
    values = exc.get("values", [])

    for item in values:
        mechanism = item.get("mechanism", {})
        value = item.get("value", "")
        if (
            mechanism.get("type", "") == "tornado"
            and mechanism.get("handled", None) is False
            and (value == "Session is disconnected" or "Invalid session" in value)
        ):
            return True
    return False


def before_send(event, hint):
    if is_tornado_session_disconnected(event):
        return None
    return sanitize_sentry_event(event, hint)


if SENTRY_ENABLED:
    print("Initializing sentry")

    sentry_sdk.init(
        dsn="https://39a03a6899dcf043dc520bc93d9e966c@sentry.meticulousespresso.com/3",
        traces_sample_rate=0.0,
        profiles_sample_rate=0.0,
        integrations=[
            AsyncioIntegration(),
        ],
        ignore_errors=[
            KeyboardInterrupt,
        ],
        send_default_pii=False,
        include_local_variables=False,
        max_breadcrumbs=0,
        before_breadcrumb=drop_breadcrumb,
        before_send=before_send,
    )


else:
    print("Skipping Sentry initialization")


def run():
    from tornado.websocket import WebSocketClosedError
    from db_migration_updater import update_db_migrations
    from log import MeticulousLogger
    from backend import main as backend_main
    from shot_manager import ShotManager

    # Add ignored errors to sentry now that the import suceeded
    if SENTRY_ENABLED:
        client = sentry_sdk.get_client()
        client.options["ignore_errors"].append(WebSocketClosedError)

    logger = MeticulousLogger.getLogger(__name__)

    try:
        try:
            ShotManager.init()
        except Exception as e:
            logger.error("Failed to initialize ShotManager", exc_info=e)
        try:
            update_db_migrations()
        except Exception as e:
            logger.error("Failed to run database migrations", exc_info=e)

        backend_main()
    except Exception as e:
        logger.exception("main() failed", exc_info=e, stack_info=True)
        exit(1)


if __name__ == "__main__":
    run()

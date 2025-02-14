import sentry_sdk
from sentry_sdk.integrations.asyncio import AsyncioIntegration

import os


BACKEND = os.getenv("BACKEND", "FIKA").upper()
SENTRY = os.getenv("SENTRY", "False").lower() in ("true", "1", "y")


def before_breadcrumb(crumb, hint):
    # Dont log subprocess breadcrumbs
    if crumb["type"] == "subprocess":
        # If we wanted to allow certain threads we could do it like this:
        # thread_name = crumb.get("data", {}).get("thread.name", None) -> e.g. MainThread or WifiAutoConnect
        # thread_name = crumb.get("message", None) -> e.g. 'nmcli device show wlan0'
        return None
    return crumb


def before_send(event, hint):
    return event


if BACKEND == "FIKA" or SENTRY:
    print("Initializing sentry")
    sentry_sdk.init(
        dsn="https://0b7872daf08aae52a8d654472bc8bb26@o4506723336060928.ingest.us.sentry.io/4507635208224768",
        # Set traces_sample_rate to 1.0 to capture 100%
        # of transactions for performance monitoring.
        traces_sample_rate=0.0,
        # Set profiles_sample_rate to 1.0 to profile 100%
        # of sampled transactions.
        # We recommend adjusting this value in production.
        profiles_sample_rate=0.0,
        integrations=[
            AsyncioIntegration(),
        ],
        ignore_errors=[
            KeyboardInterrupt,
        ],
        before_breadcrumb=before_breadcrumb,
        before_send=before_send,
    )
else:
    print("Skipping Sentry initialization")


def run():
    from tornado.websocket import WebSocketClosedError
    from db_migration_updater import update_db_migrations
    from log import MeticulousLogger
    from backend import main as backend_main

    # Add ignored errors to sentry now that the import suceeded
    client = sentry_sdk.get_client()
    client.options["ignore_errors"].append(WebSocketClosedError)

    logger = MeticulousLogger.getLogger(__name__)

    try:
        update_db_migrations()
        backend_main()
    except Exception as e:
        logger.exception("main() failed", exc_info=e, stack_info=True)
        exit(1)


if __name__ == "__main__":
    run()

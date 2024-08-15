import sentry_sdk
from sentry_sdk.integrations.asyncio import AsyncioIntegration

import os

BACKEND = os.getenv("BACKEND", "FIKA").upper()
SENTRY = os.getenv("SENTRY", "False").lower() in ("true", "1", "y")

if BACKEND == "FIKA" or SENTRY:
    print("Initializing sentry")
    sentry_sdk.init(
        dsn="https://0b7872daf08aae52a8d654472bc8bb26@o4506723336060928.ingest.us.sentry.io/4507635208224768",
        # Set traces_sample_rate to 1.0 to capture 100%
        # of transactions for performance monitoring.
        traces_sample_rate=1.0,
        # Set profiles_sample_rate to 1.0 to profile 100%
        # of sampled transactions.
        # We recommend adjusting this value in production.
        profiles_sample_rate=0.0,
        integrations=[
            AsyncioIntegration(),
        ],
    )
else:
    print("Skipping Sentry initialization")


from log import MeticulousLogger  # noqa: E402
from backend import main as backend_main  # noqa: E402

logger = MeticulousLogger.getLogger(__name__)

if __name__ == "__main__":
    try:
        backend_main()
    except Exception as e:
        logger.exception("main() failed", exc_info=e, stack_info=True)
        exit(1)

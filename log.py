import gzip
import logging
import logging.handlers
import os
import shutil
import coloredlogs

LOGLEVEL = os.getenv("LOGLEVEL", "DEBUG").upper()
LOG_PATH = os.getenv("LOG_PATH", "/meticulous-user/logs")

logformat = "%(asctime)s %(name)s %(levelname)s %(message)s"
coloredlogs.install(fmt=logformat, level=LOGLEVEL, milliseconds=True)

logging.basicConfig()


class MeticulousLogger:
    _rh = None
    _f = None
    _sh = None

    FORCE_STDOUT_LOG = os.getenv("FORCE_STDOUT_LOG", "False").lower() in (
        "true",
        "1",
        "y",
    )

    def setFileLogLevel(level):
        if MeticulousLogger._rh is None:
            MeticulousLogger._createHandler()
        MeticulousLogger._rh.setLevel(level)

    # Callback when a new log is created
    def cb_logname(name):
        return name + ".gz"

    # function called to rotatethe log at a certain size or time
    def cb_logrotate(source, dest):
        with open(source, "rb") as f_in:
            with gzip.open(dest, "wb") as f_out:
                shutil.copyfileobj(f_in, f_out)
        os.remove(source)

    def _createHandler():
        MB = 1024 * 1024
        # Create directory for the logfiles if it doesn't exist
        os.makedirs(LOG_PATH, exist_ok=True)
        logfilePath = os.path.join(LOG_PATH, "backend.log")

        MeticulousLogger._rh = logging.handlers.RotatingFileHandler(
            logfilePath, maxBytes=200 * MB, backupCount=10
        )
        MeticulousLogger._rh.rotator = MeticulousLogger.cb_logrotate
        MeticulousLogger._rh.namer = MeticulousLogger.cb_logname

        MeticulousLogger._f = logging.Formatter(logformat)
        MeticulousLogger._rh.setFormatter(MeticulousLogger._f)
        MeticulousLogger._rh.doRollover()
        MeticulousLogger._sh = logging.StreamHandler()

    def getLogger(name):
        if MeticulousLogger._rh is None:
            MeticulousLogger._createHandler()

        logger = logging.getLogger(name=name)
        if MeticulousLogger._rh not in logger.handlers:
            logger.addHandler(MeticulousLogger._rh)
        # This will lead to double logging on some systems, so it is disabled by default
        if (
            MeticulousLogger.FORCE_STDOUT_LOG
            and MeticulousLogger._sh not in logger.handlers
        ):
            logger.addHandler(MeticulousLogger._sh)
        logger.setLevel(LOGLEVEL)
        return logger

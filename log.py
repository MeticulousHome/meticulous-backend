import gzip
import logging
import logging.handlers
import os
import shutil

import coloredlogs

LOGLEVEL = os.getenv("LOGLEVEL", "DEBUG").upper()
logformat = "%(asctime)s %(name)s %(levelname)s %(message)s"
coloredlogs.install(fmt=logformat, level=LOGLEVEL, milliseconds=True)

logging.basicConfig()


class MeticulousLogger:
    _sh = None

    FORCE_STDOUT_LOG = os.getenv("FORCE_STDOUT_LOG", "False").lower() in (
        "true",
        "1",
        "y",
    )

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
        MeticulousLogger._sh = logging.StreamHandler()

    def getLogger(name):
        if MeticulousLogger._sh is None:
            MeticulousLogger._createHandler()

        logger = logging.getLogger(name=name)

        # This will lead to double logging on some systems, so it is disabled by default
        if MeticulousLogger.FORCE_STDOUT_LOG and MeticulousLogger._sh not in logger.handlers:
            logger.addHandler(MeticulousLogger._sh)
        logger.setLevel(LOGLEVEL)
        return logger

    @staticmethod
    def add_logging_handler(handler: logging.Handler, logger_name: str | None = None):
        if handler is None:
            return
        if logger_name is not None:
            target_logger = logging.Logger.manager.loggerDict.get(logger_name, None)
            if isinstance(target_logger, logging.Logger):
                target_logger.addHandler(handler)
                return
        for _, logger in logging.Logger.manager.loggerDict.items():
            if not isinstance(logger, logging.Logger):
                continue
            logger.addHandler(handler)

    @staticmethod
    def remove_logging_handler(handler: logging.Handler, logger_name: str | None = None):
        if handler is None:
            return
        if logger_name is not None:
            target_logger = logging.Logger.manager.loggerDict.get(logger_name, None)
            if isinstance(target_logger, logging.Logger):
                target_logger.removeHandler(handler)
                return
        for _, logger in logging.Logger.manager.loggerDict.items():
            if not isinstance(logger, logging.Logger):
                continue
            logger.removeHandler(handler)

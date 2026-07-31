import gzip
import logging
import logging.handlers
import os
import shutil
import coloredlogs

from log_redaction_filter import LogRedactionFilter

LOGLEVEL = os.getenv("LOGLEVEL", "DEBUG").upper()
logformat = "%(asctime)s %(name)s %(levelname)s %(message)s"
coloredlogs.install(fmt=logformat, level=LOGLEVEL, milliseconds=True)

logging.basicConfig()

# One instance, deliberately with no switch to turn it off: a privacy control
# that can be disabled is a liability. Installed in two places, because neither
# alone is enough:
#
#   * On every logger built here, which is every first-party logger. A logger
#     filter runs inside Logger.handle() before callHandlers(), so it is the only
#     placement that reaches Sentry's LoggingIntegration deterministically.
#   * On the root logger's handlers, which catch third-party loggers
#     (dbus_next, aiohttp, zeroconf, alembic) that never come through getLogger.
#
# A record that takes both routes is marked and only worked once.
_REDACTION_FILTER = LogRedactionFilter()

for _root_handler in logging.getLogger().handlers:
    if _REDACTION_FILTER not in _root_handler.filters:
        _root_handler.addFilter(_REDACTION_FILTER)


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
        MeticulousLogger._sh.addFilter(_REDACTION_FILTER)

    def getLogger(name):
        if MeticulousLogger._sh is None:
            MeticulousLogger._createHandler()

        logger = logging.getLogger(name=name)

        if _REDACTION_FILTER not in logger.filters:
            logger.addFilter(_REDACTION_FILTER)

        # This will lead to double logging on some systems, so it is disabled by default
        if MeticulousLogger.FORCE_STDOUT_LOG and MeticulousLogger._sh not in logger.handlers:
            logger.addHandler(MeticulousLogger._sh)
        logger.setLevel(LOGLEVEL)
        return logger

    @staticmethod
    def set_redaction_value_provider(provider):
        """Give the redaction filter access to live config values.

        Called by config.py once its singleton exists. See
        LogRedactionFilter.set_value_provider.
        """
        _REDACTION_FILTER.set_value_provider(provider)

    @staticmethod
    def invalidate_redaction_values():
        """Drop the filter's cached view of the config. Called from config.save()."""
        _REDACTION_FILTER.invalidate()

    @staticmethod
    def add_logging_handler(handler: logging.Handler, logger_name: str | None = None):
        if handler is None:
            return
        # The shot-debug handler writes log text into files that ship inside the
        # bug report, so it has to be filtered even if it is ever attached to a
        # logger that did not come through getLogger.
        if _REDACTION_FILTER not in handler.filters:
            handler.addFilter(_REDACTION_FILTER)
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

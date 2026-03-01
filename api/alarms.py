import time
import os
from log import MeticulousLogger
import math
from enum import Enum
from named_thread import NamedThread

from images.notificationImages.base64 import WARNING_TRIANGLE_IMAGE
from notifications import Notification, NotificationManager, NotificationResponse

ALARMS_PATH = os.getenv("ALARMS_PATH", "/meticulous-user/syslog/alarms")

logger = MeticulousLogger.getLogger(name=__name__)


class AlarmType(Enum):
    MOTOR_STRESSED = "motor_stress_alarm"
    ESP_RESTART = "esp_restart_alarm"
    ESP_DISCONNECTED = "esp_disconected_alarm"

    @staticmethod
    def from_file_name(name):
        match name:
            case "motor_stress_alarm":
                return AlarmType.MOTOR_STRESSED
            case "esp_restart_alarm":
                return AlarmType.ESP_RESTART
            case "esp_disconected_alarm":
                return AlarmType.ESP_DISCONNECTED
            case _:
                return None


class Alarm:

    def __init__(self, type: AlarmType, end_time: float | None):
        self.start_time = time.time()
        self.end_time = end_time
        self.type = type
        self.alarm_path = os.path.join(ALARMS_PATH, self.type.value)

    def create_file(self):
        if not os.path.exists(ALARMS_PATH):
            os.makedirs(ALARMS_PATH)

        try:
            with open(self.alarm_path, "w") as alarm_file:
                alarm_file.write(str(self.end_time))
            return True
        except Exception as e:
            logger.warning(
                f"Cannot write {self.type.value} file, the alarm will be lost if the backend stops: {e}"
            )
            return False

    def remove_file(self):
        logger.debug(f"clearing {self.type.value} file")
        if not os.path.exists(self.alarm_path):
            logger.warning(f"{self.type.value} files does not exist")
            return
        try:
            os.remove(self.alarm_path)
        except Exception as e:
            logger.error(f"Cannot remove {self.type.value} file: {e}")


class AlarmManager:
    alarms: dict[str, Alarm] = {}
    thread: NamedThread = None
    initialized = False

    @staticmethod
    def init():
        if not os.path.exists(ALARMS_PATH):
            try:
                logger.info("Creating alarms directory")
                os.makedirs(ALARMS_PATH, exist_ok=True)
            except Exception as e:
                logger.error(
                    f"Cannot create alarms directory, alarms set will be lost on backend restart: {e}"
                )

        logger.info("Loading alarms from disk")
        # load all alarms written in disk
        for alarm_file in os.listdir(ALARMS_PATH):
            type = AlarmType.from_file_name(alarm_file)
            if type is not None:
                logger.debug(f"Alarm file found for {type.value}")
                alarm_file_path = os.path.join(ALARMS_PATH, alarm_file)
                try:
                    with open(alarm_file_path, "r") as alarm_file:
                        new_alarm = Alarm(type, float(alarm_file.read().strip()))
                    AlarmManager.alarms.setdefault(type.value, new_alarm)
                except Exception as e:
                    logger.warning(
                        f"Cannot read file {type.value}, clearing alarm: {e}"
                    )
                    try:
                        os.remove(alarm_file_path)
                    except Exception as e:
                        logger.error(f"Cannot remove file {type.value}: {e}")
                    finally:
                        AlarmManager.alarms[type.value] = None

        # start the monitoring thread
        AlarmManager.start_thread()
        AlarmManager.initialized = True

    @staticmethod
    def start_thread():
        if AlarmManager.thread is None:
            AlarmManager.thread = NamedThread(
                name="alarm manager", target=AlarmManager.monitoring_task
            )
            AlarmManager.thread.start()
        else:
            logger.warning("Alarm manager task has already started")

    @staticmethod
    def monitoring_task():
        logger.debug("starting alarm manager task")

        while True:
            # check every alarm in the list, clear them if their end_time has passed
            now = time.time()
            to_remove: list[str] = []
            for type, alarm in AlarmManager.alarms.items():
                if alarm is None:
                    continue
                if math.isfinite(alarm.end_time) and alarm.end_time < now:
                    alarm.remove_file()
                    to_remove.append(type)

            for type in to_remove:
                AlarmManager.alarms[type] = None

            time.sleep(1)

    @staticmethod
    def set_alarm(
        type: AlarmType, end_time: float | None, force: bool, quiet: bool = False
    ):
        new_alarm: Alarm = Alarm(type, end_time if end_time is not None else -math.inf)
        msg = ""
        img = WARNING_TRIANGLE_IMAGE

        match type:
            case AlarmType.MOTOR_STRESSED:
                msg = f"Brewing paused because of high strain in the motor. Let the machine rest for {math.ceil((end_time - time.time())/60.0) if math.isfinite(end_time) else 10} mins and use a coarser grind before trying again"
            case AlarmType.ESP_RESTART:
                msg = (
                    "Digital controller seems to be unresponsive, buttons are disabled."
                )
            case AlarmType.ESP_DISCONNECTED:
                msg = "Digital controller seems disconnected, buttons are disabled"

        if not AlarmManager.initialized:
            logger.warning("The alarm manager is not initialized")

        if AlarmManager.thread is None or not AlarmManager.thread.is_alive():
            logger.warning(
                "The alarm manager thread is not alive, alarms will not end on time"
            )

        if not os.path.exists(new_alarm.alarm_path) or force:
            if end_time is not None:
                new_alarm.create_file()
            else:
                logger.info(
                    f"setting alarm {type.value} with the duration of the session"
                )

            AlarmManager.alarms[type.value] = new_alarm
            if not quiet:
                AlarmManager._notify_user(message=msg, image=img)

        else:
            try:
                with open(new_alarm.alarm_path, "r") as alarm_file:
                    new_alarm.end_time = float(alarm_file.read().strip())
                AlarmManager.alarms[type.value] = new_alarm
                duration_alarm_str = (
                    f"{(new_alarm.end_time - time.time()):.2f}s"
                    if not math.isinf(new_alarm.end_time)
                    else "Never"
                )
                logger.warning(
                    f"Alarm was already set, will end in: {duration_alarm_str}"
                )
                logger.warning("You can force the alarm to overwrite it")
            except Exception as e:
                logger.warning(
                    f"Cannot read {new_alarm.type.value} file, clearing alarm: {e}"
                )
                new_alarm.remove_file()
                AlarmManager.alarms[type.value] = None

    @staticmethod
    def clear_alarm(type: AlarmType):
        alarm_to_clear = AlarmManager.alarms.get(type.value, None)
        if alarm_to_clear is not None:
            logger.debug(f"Removing {type.value}")
            alarm_to_clear.remove_file()
            AlarmManager.alarms[type.value] = None

    @staticmethod
    def _notify_user(message, image):
        # TODO: define alarm severity if needed and notify based on that
        NotificationManager.add_notification(
            Notification(
                message=message, image=image, responses=[NotificationResponse.OK]
            )
        )

    @staticmethod
    def is_alarm_set(type: AlarmType):
        alarm = AlarmManager.alarms.get(type.value, None)
        return alarm.end_time if alarm else None

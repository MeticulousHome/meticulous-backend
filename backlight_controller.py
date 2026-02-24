import time
import threading

from machine import Machine
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

BRIGHTNESS_FILE = "/sys/class/backlight/backlight/brightness"
MAX_BRIGHTNESS_FILE = "/sys/class/backlight/backlight/max_brightness"

MIN_BRIGHTNESS = 0.33


class BacklightController:
    _adjust_thread = None
    _MAX_BRIGHTNESS = None

    @staticmethod
    def _get_current_raw_brightness():
        with open(BRIGHTNESS_FILE, "r") as f:
            return int(f.read().strip())

    @staticmethod
    def _get_max_raw_brightness():
        if BacklightController._MAX_BRIGHTNESS:
            return BacklightController._MAX_BRIGHTNESS

        with open(MAX_BRIGHTNESS_FILE, "r") as f:
            BacklightController._MAX_BRIGHTNESS = int(f.read().strip())
            return BacklightController._MAX_BRIGHTNESS

    @staticmethod
    def _set_raw_brightness(value):
        if value < 0:
            value = 0
        if value > BacklightController._get_max_raw_brightness():
            value = BacklightController._get_max_raw_brightness()
        with open(BRIGHTNESS_FILE, "w") as f:
            f.write(str(value))

    # Linear interpolation
    @staticmethod
    def linear_interpolation(start, end, steps):
        step_size = (end - start) / steps
        for i in range(steps):
            yield start + step_size * i

    # Quadratic interpolation (curve)
    @staticmethod
    def curve_interpolation(start, end, steps):
        for i in range(steps):
            t = i / steps
            yield start + (end - start) * (t**2)

    @staticmethod
    def stop_adjust_thread():
        if (
            BacklightController._adjust_thread
            and BacklightController._adjust_thread.is_alive()
        ):
            BacklightController._adjust_thread.do_run = False
            BacklightController._adjust_thread.join()

    @staticmethod
    def adjust_brightness_thread(
        target_percent, interpolation="linear", steps_per_second=50, target_time=None
    ):
        t = threading.current_thread()
        current_brightness = BacklightController._get_current_raw_brightness()
        target_brightness = round(
            BacklightController._get_max_raw_brightness() * target_percent
        )

        if interpolation == "linear":
            interpolator = BacklightController.linear_interpolation(
                current_brightness,
                target_brightness,
                int(steps_per_second * target_time),
            )
        elif interpolation == "curve":
            interpolator = BacklightController.curve_interpolation(
                current_brightness,
                target_brightness,
                int(steps_per_second * target_time),
            )
        else:
            raise ValueError("Interpolation must be 'linear' or 'curve'")

        time_per_step = (1 / steps_per_second) if target_time is not None else (0.013)

        start = time.time()
        for brightness in interpolator:
            if getattr(t, "do_run", True) is False:
                break
            set_start = time.time()
            BacklightController._set_raw_brightness(int(brightness))
            time_writing = time.time() - set_start
            if time_per_step > time_writing:
                time.sleep(time_per_step - time_writing)
        if getattr(t, "do_run", True) is True:
            BacklightController._set_raw_brightness(int(target_brightness))
        logger.info(
            f"screen dimmed to {BacklightController._get_current_raw_brightness()}%, took {time.time() - start}"
        )

    @staticmethod
    def adjust_brightness(
        target_percent, interpolation="linear", steps_per_second=50, target_time=None
    ):
        if target_percent < 0:
            target_percent = 0
        if target_percent > 1:
            target_percent = 1

        logger.info(f"Adjusting Brightness to {target_percent}")

        if Machine.emulated:
            return

        BacklightController.stop_adjust_thread()
        BacklightController._adjust_thread = threading.Thread(
            target=BacklightController.adjust_brightness_thread,
            args=(target_percent, interpolation, steps_per_second, target_time),
        )
        BacklightController._adjust_thread.start()

    @staticmethod
    def dim(target_percent, interpolation="curve", target_time=1.0):
        try:
            BacklightController.adjust_brightness(
                target_percent,
                interpolation=interpolation,
                steps_per_second=75,
                target_time=target_time,
            )
        except Exception as e:
            logger.warning(f"An error occurred: {e}")

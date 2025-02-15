import time
import threading

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
    def adjust_brightness_thread(target_percent, interpolation="linear", steps=50):
        t = threading.currentThread()
        current_brightness = BacklightController._get_current_raw_brightness()
        target_brightness = round(
            BacklightController._get_max_raw_brightness() * target_percent
        )

        if interpolation == "linear":
            interpolator = BacklightController.linear_interpolation(
                current_brightness, target_brightness, steps
            )
        elif interpolation == "curve":
            interpolator = BacklightController.curve_interpolation(
                current_brightness, target_brightness, steps
            )
        else:
            raise ValueError("Interpolation must be 'linear' or 'curve'")

        for brightness in interpolator:
            if getattr(t, "do_run", True) is False:
                break
            BacklightController._set_raw_brightness(int(brightness))
            time.sleep(0.01)

    @staticmethod
    def adjust_brightness(target_percent, interpolation="linear", steps=50):
        if target_percent < 0:
            target_percent = 0
        if target_percent > 1:
            target_percent = 1

        logger.info(f"Adjusting Brightness to {target_percent}")

        BacklightController.stop_adjust_thread()
        BacklightController._adjust_thread = threading.Thread(
            target=BacklightController.adjust_brightness_thread,
            args=(target_percent, interpolation, steps),
        )
        BacklightController._adjust_thread.start()

    @staticmethod
    def dim_down():
        target_percent = MIN_BRIGHTNESS
        try:
            BacklightController.adjust_brightness(
                target_percent, interpolation="curve", steps=300
            )
        except Exception as e:
            logger.warning(f"An error occurred: {e}")

    @staticmethod
    def dim_up():
        target_percent = 1.0
        try:
            BacklightController.adjust_brightness(
                target_percent, interpolation="linear", steps=20
            )
        except Exception as e:
            logger.warning(f"An error occurred: {e}")

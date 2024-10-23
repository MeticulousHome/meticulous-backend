import time
import threading

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

BRIGHTNESS_FILE = "/sys/class/backlight/backlight/brightness"
MAX_BRIGHTNESS = 4095
MIN_BRIGHTNESS = 512


class BacklightController:
    _adjust_thread = None

    def get_current_brightness():
        with open(BRIGHTNESS_FILE, "r") as f:
            return int(f.read().strip())

    def set_brightness(value):
        with open(BRIGHTNESS_FILE, "w") as f:
            f.write(str(value))

    # Linear interpolation
    def linear_interpolation(start, end, steps):
        step_size = (end - start) / steps
        for i in range(steps):
            yield start + step_size * i

    # Quadratic interpolation (curve)
    def curve_interpolation(start, end, steps):
        for i in range(steps):
            t = i / steps
            yield start + (end - start) * (t**2)

    def stop_adjust_thread():
        if (
            BacklightController._adjust_thread
            and BacklightController._adjust_thread.is_alive()
        ):
            BacklightController._adjust_thread.do_run = False
            BacklightController._adjust_thread.join()

    def adjust_brightness_thread(target_brightness, interpolation="linear", steps=50):
        t = threading.currentThread()
        current_brightness = BacklightController.get_current_brightness()

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
            BacklightController.set_brightness(int(brightness))
            time.sleep(0.01)

    def adjust_brightness(target_brightness, interpolation="linear", steps=50):
        BacklightController.stop_adjust_thread()
        BacklightController._adjust_thread = threading.Thread(
            target=BacklightController.adjust_brightness_thread,
            args=(target_brightness, interpolation, steps),
        )
        BacklightController._adjust_thread.start()

    def dim_down():
        target_brightness = MIN_BRIGHTNESS
        try:
            BacklightController.adjust_brightness(
                target_brightness, interpolation="curve", steps=300
            )
        except Exception as e:
            logger.warning(f"An error occurred: {e}")

    def dim_up():
        target_brightness = MAX_BRIGHTNESS
        try:
            BacklightController.adjust_brightness(
                target_brightness, interpolation="linear", steps=20
            )
        except Exception as e:
            logger.warning(f"An error occurred: {e}")

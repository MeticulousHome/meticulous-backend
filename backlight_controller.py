import time
import threading

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

BRIGHTNESS_FILE = "/sys/class/backlight/backlight/brightness"
MAX_BRIGHTNESS = 4095
MIN_BRIGHTNESS = 512


class BacklightController:
    _adjust_thread = None

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

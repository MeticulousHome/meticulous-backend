from collections import deque
from esp_serial.data import SensorData, ShotData
from log import MeticulousLogger
import time
import os
from threading import Thread

INTEGRATION_WINDOW_TIME = 600
CONSTANT_DISSIPATED_ENERGY = 0
logger = MeticulousLogger.getLogger(name=__name__)

MOTOR_ENERGY_PATH = "/meticulous-user/energy"

# energy consumtion calculator
# numerically integrates the value provided in real time using the timestamp deltas as dT
# along an integration window of INTEGRATION_WINDOW_TIME seconds


class EnergyCalculator:
    def __init__(self, integration_window_seconds, name):
        self.window_seconds = integration_window_seconds
        self.history: deque[tuple[float, float]] = deque()
        self.total_energy: float = 0.0
        self.save_thread: Thread = None
        self.name: str = f"{name}_energy"
        self.save_file_path = os.path.join(MOTOR_ENERGY_PATH, self.name)

        # read the saved data from the nvs if it exist
        if os.path.exists(self.save_file_path):
            try:
                with open(self.save_file_path, "r") as e_file:
                    data = e_file.read()
                    self.total_energy = float(data)
            except Exception as e:
                logger.warning(f"cannot read motor energy stored file: {e}")
        logger.debug(f"starting {self.name}: {self.total_energy}")

        # start saving data thread
        self.start_data_save_thread()

    def calculate_motor_energy(self, sensors: SensorData, shot: ShotData):
        """
        numerically integrates the value provided in real time using the timestamp deltas as dT
        along an integration window of INTEGRATION_WINDOW_TIME seconds

        :param sensors: object holding the sensor information

        """

        now = time.monotonic()

        flow = max(abs(shot.flow if shot else 0.0), 0.005)
        power = abs(sensors.motor_power / 100.0) * abs(sensors.motor_current) / flow
        if len(self.history) == 0:
            self.history.append((0, now))
            return float(0.0)

        last_timestamp = self.history[-1][1]
        dT = now - last_timestamp

        if dT < 0:
            dT = 0.0

        added_energy = power * dT * abs(shot.pressure) if shot else power * dT
        self.history.append((added_energy, now))

        self.total_energy += added_energy
        self.total_energy = (
            self.total_energy - CONSTANT_DISSIPATED_ENERGY
            if self.total_energy - CONSTANT_DISSIPATED_ENERGY >= 0
            else self.total_energy
        )

        energy_to_remove = 0.0
        while self.history and (now - self.history[0][1] > self.window_seconds):
            energy_to_remove += self.history.popleft()[0]

        self.total_energy -= energy_to_remove

        # remove oldest energy contribution
        return self.total_energy

    def restart(self):
        self.history.clear()
        self.total_energy = 0.0
        self.window_seconds = INTEGRATION_WINDOW_TIME

    def set_window_time(self, new_window_time: float | int):
        if not isinstance(new_window_time, (float, int)):
            raise TypeError(f"{new_window_time} is not a number")
        self.window_seconds = new_window_time

        if self.history:
            now = self.history[-1][1]
            while self.history and now - self.history[0][1] > self.window_seconds:
                self.total_energy -= self.history.popleft()[0]

    def save_data_nvs(self):
        if not os.path.exists(MOTOR_ENERGY_PATH):
            os.makedirs(MOTOR_ENERGY_PATH, exist_ok=True)
        with open(self.save_file_path, "w") as e_file:
            e_file.write(str(self.total_energy))

    @staticmethod
    def save_data_thread(instance):
        if not isinstance(instance, EnergyCalculator):
            return
        logger.log("save_data_thread started")
        # infinite loop, save data every second
        while True:
            instance.save_data_nvs()
            time.sleep(1)

    def start_data_save_thread(self):
        if self.save_thread is None:
            self.save_thread = Thread(
                target=EnergyCalculator.save_data_thread, args=(self,)
            )
            self.save_thread.start()
        else:
            logger.warning(f"{self.name} data saving thread has already started")


motor_energy_calculator = EnergyCalculator(INTEGRATION_WINDOW_TIME, "motor")

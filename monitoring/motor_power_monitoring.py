from collections import deque
from esp_serial.data import SensorData, ShotData
from log import MeticulousLogger
import time

INTEGRATION_WINDOW_TIME = 360
CONSTANT_DISSIPATED_ENERGY = 0
logger = MeticulousLogger.getLogger(name=__name__)

# energy consumtion calculator
# numerically integrates the value provided in real time using the timestamp deltas as dT
# along an integration window of INTEGRATION_WINDOW_TIME seconds


class EnergyCalculator:
    def __init__(self, integration_window_seconds):
        self.window_seconds = integration_window_seconds
        self.history: deque[tuple[float, float]] = deque()
        self.total_energy: float = 0.0

    def calculate_motor_energy(self, sensors: SensorData, shot: ShotData):
        """
        numerically integrates the value provided in real time using the timestamp deltas as dT
        along an integration window of INTEGRATION_WINDOW_TIME seconds

        :param sensors: object holding the sensor information

        """

        now = time.monotonic()
        power = abs(sensors.motor_power / 100.0) * abs(sensors.motor_current)
        if len(self.history) == 0:
            self.history.append((0, now))
            return float(0.0)

        last_timestamp = self.history[-1][1]
        dT = now - last_timestamp

        if dT < 0:
            dT = 0.0

        added_energy = (
            power * dT * abs(shot.pressure) if shot and shot.pressure else power * dT
        )
        self.history.append((added_energy, now))

        self.total_energy += added_energy
        self.total_energy = (
            self.total_energy - CONSTANT_DISSIPATED_ENERGY
            if self.total_energy - CONSTANT_DISSIPATED_ENERGY >= 0
            else self.total_energy
        )

        energy_to_remove = 0.0
        while self.history and (now - self.history[0][1] > self.window_seconds):
            energy_to_remove -= self.history.popleft()[0]

        self.total_energy -= energy_to_remove

        # remove oldest energy contribution
        logger.debug(
            f"energy = {self.total_energy} -> [+ {added_energy}] [- {energy_to_remove}] <---> mp: {sensors.motor_power}, mc: {sensors.motor_current}, dt: {dT}, p: {shot.pressure if shot else 'None'}"
        )
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


motor_energy_calculator = EnergyCalculator(INTEGRATION_WINDOW_TIME)

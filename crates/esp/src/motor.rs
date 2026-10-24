//! `monitoring/motor_power_monitoring.py`: the motor's energy over a sliding
//! window, which stops a shot when the motor is stressed.
//!
//! Only the calculation lives here. The Python also persists the total, and
//! the backend keeps doing that:
//!
//! - file: `$MOTOR_ENERGY_PATH/motor_energy`, `MOTOR_ENERGY_PATH` defaulting
//!   to `/meticulous-user/syslog/energy`;
//! - at startup (module import), if the file exists: read it, and call
//!   [`EnergyCalculator::restore`] with its text and `time.time()` minus its
//!   mtime;
//! - from then on, once a second (a thread started at import): create the
//!   directory if missing and overwrite the file with
//!   [`EnergyCalculator::saved_text`] (`str(total_energy)`, no newline).

use std::collections::VecDeque;

use pyfmt::{py_float, repr_f64};

use crate::pynum::{PyNum, py_max, py_strip};

/// `INTEGRATION_WINDOW_TIME`, seconds.
pub const INTEGRATION_WINDOW_TIME: i64 = 600;
/// `CONSTANT_DISSIPATED_ENERGY`.
pub const CONSTANT_DISSIPATED_ENERGY: i64 = 0;
/// `MAX_ENERGY_ALLOWED`: `Machine.stopMotorIfHot` raises the
/// `MOTOR_STRESSED` alarm (10 minutes) at or above it, and ends a running
/// profile with `home`.
pub const MAX_ENERGY_ALLOWED: i64 = 25000;
/// `APPROX_SAMPLE_RATE`: ESP updates per second.
pub const APPROX_SAMPLE_RATE: i64 = 10;
/// The default of `MOTOR_ENERGY_PATH`.
pub const DEFAULT_MOTOR_ENERGY_PATH: &str = "/meticulous-user/syslog/energy";
/// The file name under `MOTOR_ENERGY_PATH` (`f"{name}_energy"`, name `motor`).
pub const MOTOR_ENERGY_FILE_NAME: &str = "motor_energy";

/// `energy >= MAX_ENERGY_ALLOWED`.
pub fn exceeds_max_energy(energy: f64) -> bool {
    energy >= MAX_ENERGY_ALLOWED as f64
}

/// `calculate_motor_energy` raised `TypeError` on a `"NaN"` string or
/// `None` in a field it does arithmetic with. In Python the exception leaves
/// `Machine.stopMotorIfHot` and ends the serial reader thread; a `Data` line
/// with `nan` flow followed by a `Sensors` line is enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("TypeError: {field} is not a number")]
pub struct NotANumber {
    pub field: &'static str,
}

/// What [`EnergyCalculator::restore`] made of the saved file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// Restored, with the energy dissipated while the machine was off taken
    /// away.
    Restored,
    /// The file's time lies in the future: restored without that
    /// compensation (logged as a warning).
    RestoredFromTheFuture,
    /// The window is not a whole number of seconds, so Python's `range()`
    /// raised after the total was restored: no history (warning).
    RestoredWithoutHistory,
    /// The text is not a float: nothing restored (warning).
    Unreadable,
}

/// `EnergyCalculator`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyCalculator {
    window_seconds: f64,
    /// `(energy, monotonic timestamp)`, oldest first.
    history: VecDeque<(f64, f64)>,
    total_energy: f64,
}

impl EnergyCalculator {
    /// `EnergyCalculator(integration_window_seconds, name)` without its file
    /// and thread. A window that is not positive becomes 1 second.
    pub fn new(integration_window_seconds: f64) -> Self {
        EnergyCalculator {
            window_seconds: if integration_window_seconds > 0.0 {
                integration_window_seconds
            } else {
                1.0
            },
            history: VecDeque::new(),
            total_energy: 0.0,
        }
    }

    /// The calculator the backend uses (`motor_energy_calculator`).
    pub fn motor() -> Self {
        EnergyCalculator::new(INTEGRATION_WINDOW_TIME as f64)
    }

    pub fn total_energy(&self) -> f64 {
        self.total_energy
    }

    pub fn window_seconds(&self) -> f64 {
        self.window_seconds
    }

    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// What the save thread writes: `str(total_energy)`.
    pub fn saved_text(&self) -> String {
        repr_f64(self.total_energy)
    }

    /// The constructor's restore from the saved file: `saved` is the file's
    /// text, `seconds_since_saved` is `time.time()` minus its mtime. The
    /// machine is assumed to have dissipated `MAX_ENERGY_ALLOWED` per window
    /// while off, and the remaining energy is spread over a synthetic window
    /// of samples ending at `now`.
    pub fn restore(&mut self, saved: &str, seconds_since_saved: f64, now: f64) -> RestoreOutcome {
        let Some(total) = py_float(py_strip(saved)) else {
            return RestoreOutcome::Unreadable;
        };
        self.total_energy = total;
        let slope = (0 - MAX_ENERGY_ALLOWED) as f64 / self.window_seconds;
        let mut outcome = RestoreOutcome::Restored;
        if seconds_since_saved > 0.0 {
            self.total_energy = py_max(self.total_energy + seconds_since_saved * slope, 0.0);
        } else {
            outcome = RestoreOutcome::RestoredFromTheFuture;
        }
        if self.window_seconds.fract() != 0.0 {
            return RestoreOutcome::RestoredWithoutHistory;
        }
        let number_of_samples = self.window_seconds as i64 * APPROX_SAMPLE_RATE;
        let energy_per_sample = self.total_energy / number_of_samples.max(1) as f64;
        for sample_idx in 0..number_of_samples {
            self.history.push_front((
                energy_per_sample,
                now - (sample_idx as f64 / APPROX_SAMPLE_RATE as f64),
            ));
        }
        outcome
    }

    /// `calculate_motor_energy(sensors, shot)`: integrates the motor's power
    /// (scaled by pressure over flow) since the previous sample and drops
    /// what left the window. `shot` is `(flow, pressure)` of
    /// `Machine.data_sensors`, `None` when there is no shot data.
    pub fn calculate_motor_energy(
        &mut self,
        motor_power: &PyNum,
        motor_current: &PyNum,
        shot: Option<(&PyNum, &PyNum)>,
        now: f64,
    ) -> Result<f64, NotANumber> {
        let flow = match shot {
            Some((flow, _)) => flow.as_f64().ok_or(NotANumber { field: "flow" })?,
            None => 0.0,
        };
        let flow = py_max(flow, 0.005);
        let motor_power = motor_power.as_f64().ok_or(NotANumber {
            field: "motor_power",
        })?;
        let motor_current = motor_current.as_f64().ok_or(NotANumber {
            field: "motor_current",
        })?;
        let power = (motor_power / 100.0).abs() * motor_current.abs() / flow;
        let Some(&(_, last_timestamp)) = self.history.back() else {
            self.history.push_back((0.0, now));
            return Ok(0.0);
        };

        let mut d_t = now - last_timestamp;
        if d_t < 0.0 {
            d_t = 0.0;
        }

        let added_energy = match shot {
            Some((_, pressure)) => {
                let pressure = pressure.as_f64().ok_or(NotANumber { field: "pressure" })?;
                power * d_t * pressure.abs()
            }
            None => power * d_t,
        };
        self.history.push_back((added_energy, now));

        self.total_energy += added_energy;
        let dissipated = CONSTANT_DISSIPATED_ENERGY as f64;
        if self.total_energy - dissipated >= 0.0 {
            self.total_energy -= dissipated;
        }

        let mut energy_to_remove = 0.0;
        while let Some(&(energy, timestamp)) = self.history.front() {
            let expired = now - timestamp > self.window_seconds;
            if !expired {
                break;
            }
            energy_to_remove += energy;
            self.history.pop_front();
        }
        self.total_energy -= energy_to_remove;
        Ok(self.total_energy)
    }

    /// `restart`: empty history and total, default window.
    pub fn restart(&mut self) {
        self.history.clear();
        self.total_energy = 0.0;
        self.window_seconds = INTEGRATION_WINDOW_TIME as f64;
    }

    /// `set_window_time`: drops, one by one, what the new window no longer
    /// covers, measured from the newest sample.
    pub fn set_window_time(&mut self, new_window_time: f64) {
        self.window_seconds = new_window_time;
        let Some(&(_, now)) = self.history.back() else {
            return;
        };
        while let Some(&(energy, timestamp)) = self.history.front() {
            let expired = now - timestamp > self.window_seconds;
            if !expired {
                break;
            }
            self.total_energy -= energy;
            self.history.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_only_starts_the_history() {
        let mut calc = EnergyCalculator::motor();
        let one = PyNum::Float(1.0);
        assert_eq!(calc.calculate_motor_energy(&one, &one, None, 5.0), Ok(0.0));
        assert_eq!(calc.history_len(), 1);
        // power = |100 / 100| * |2| / max(0, 0.005), over 1 s, times |pressure|.
        let energy = calc
            .calculate_motor_energy(
                &PyNum::Float(100.0),
                &PyNum::Float(2.0),
                Some((&PyNum::Int(0), &PyNum::Float(-2.0))),
                6.0,
            )
            .expect("numbers");
        assert_eq!(energy, 1.0 * 2.0 / 0.005 * 1.0 * 2.0);
        assert!(exceeds_max_energy(25000.0));
        assert!(!exceeds_max_energy(24999.9));
    }

    #[test]
    fn nan_string_flow_is_a_type_error() {
        let mut calc = EnergyCalculator::motor();
        let one = PyNum::Float(1.0);
        assert_eq!(
            calc.calculate_motor_energy(&one, &one, Some((&PyNum::NanString, &one)), 1.0),
            Err(NotANumber { field: "flow" })
        );
        // The pressure is only used once there is a previous sample.
        assert!(
            calc.calculate_motor_energy(&one, &one, Some((&one, &PyNum::NanString)), 1.0)
                .is_ok()
        );
        assert_eq!(
            calc.calculate_motor_energy(&one, &one, Some((&one, &PyNum::NanString)), 2.0),
            Err(NotANumber { field: "pressure" })
        );
    }
}

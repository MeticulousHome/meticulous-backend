//! `backlight_controller.py`: fade the display's sysfs backlight to a
//! fraction of its maximum on a background thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;

use crate::os_update::python_round;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interpolation {
    Linear,
    Curve,
}

pub struct BacklightController {
    brightness_file: PathBuf,
    max_brightness_file: PathBuf,
    max_brightness: Mutex<Option<i64>>,
    adjust: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
}

/// A JSON number or bool as Python arithmetic sees it.
fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

impl BacklightController {
    pub fn new(sysfs_dir: PathBuf) -> BacklightController {
        BacklightController {
            brightness_file: sysfs_dir.join("brightness"),
            max_brightness_file: sysfs_dir.join("max_brightness"),
            max_brightness: Mutex::new(None),
            adjust: Mutex::new(None),
        }
    }

    /// `BacklightController.dim`: a fade of `target_time` seconds at 75
    /// steps per second. `leave_hardware_alone` is `Machine.emulated` (and
    /// the faked hardware).
    pub fn dim(
        self: &Arc<Self>,
        target_percent: &Value,
        interpolation: &Value,
        target_time: &Value,
        leave_hardware_alone: bool,
    ) {
        let Some(mut target) = as_number(target_percent) else {
            tracing::warn!(
                "An error occurred: '<' not supported between instances of '{}' and 'int'",
                python_type(target_percent)
            );
            return;
        };
        target = target.clamp(0.0, 1.0);
        tracing::info!("Adjusting Brightness to {}", pyfmt::repr_f64(target));
        if leave_hardware_alone {
            return;
        }
        self.stop_adjust_thread();
        let interpolation = match interpolation.as_str() {
            Some("linear") => Some(Interpolation::Linear),
            Some("curve") => Some(Interpolation::Curve),
            _ => None,
        };
        let target_time = as_number(target_time);
        let stop = Arc::new(AtomicBool::new(false));
        let controller = Arc::clone(self);
        let thread_stop = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("backlight".into())
            .spawn(move || {
                if let Err(error) =
                    controller.adjust(target, interpolation, 75, target_time, &thread_stop)
                {
                    // The Python thread dies with the exception.
                    tracing::warn!("Backlight adjustment failed: {error}");
                }
            });
        match spawned {
            Ok(handle) => *self.adjust.lock() = Some((stop, handle)),
            Err(error) => tracing::warn!("An error occurred: {error}"),
        }
    }

    fn stop_adjust_thread(&self) {
        if let Some((stop, handle)) = self.adjust.lock().take() {
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
    }

    /// Wait for the running fade to end.
    #[cfg(test)]
    pub fn wait(&self) {
        if let Some((_, handle)) = self.adjust.lock().take() {
            let _ = handle.join();
        }
    }

    #[cfg(test)]
    pub fn is_adjusting(&self) -> bool {
        self.adjust.lock().is_some()
    }

    fn current_raw_brightness(&self) -> Result<i64, String> {
        read_int(&self.brightness_file)
    }

    fn max_raw_brightness(&self) -> Result<i64, String> {
        let mut cached = self.max_brightness.lock();
        if let Some(max) = *cached {
            return Ok(max);
        }
        let max = read_int(&self.max_brightness_file)?;
        if max != 0 {
            *cached = Some(max);
        }
        Ok(max)
    }

    fn set_raw_brightness(&self, value: i64) -> Result<(), String> {
        let value = value.max(0).min(self.max_raw_brightness()?);
        std::fs::write(&self.brightness_file, value.to_string()).map_err(|e| e.to_string())
    }

    /// `adjust_brightness_thread`.
    fn adjust(
        &self,
        target_percent: f64,
        interpolation: Option<Interpolation>,
        steps_per_second: u32,
        target_time: Option<f64>,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let current = self.current_raw_brightness()? as f64;
        let target = python_round(self.max_raw_brightness()? as f64 * target_percent);
        let interpolation =
            interpolation.ok_or("Interpolation must be 'linear' or 'curve'".to_string())?;
        let target_time = target_time.ok_or("unsupported operand type(s) for *".to_string())?;
        // int(): truncation toward zero.
        let steps = (f64::from(steps_per_second) * target_time).trunc() as i64;
        let start = Instant::now();
        let time_per_step = Duration::from_secs_f64(1.0 / f64::from(steps_per_second));
        if interpolation == Interpolation::Linear && steps == 0 {
            return Err("division by zero".into());
        }
        for i in 0..steps.max(0) {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let value = match interpolation {
                Interpolation::Linear => {
                    current + (target as f64 - current) / steps as f64 * i as f64
                }
                Interpolation::Curve => {
                    let t = i as f64 / steps as f64;
                    current + (target as f64 - current) * (t * t)
                }
            };
            let written = Instant::now();
            self.set_raw_brightness(value.trunc() as i64)?;
            if let Some(rest) = time_per_step.checked_sub(written.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        if !stop.load(Ordering::Relaxed) {
            self.set_raw_brightness(target)?;
        }
        tracing::info!(
            "screen dimmed to {}%, took {}",
            self.current_raw_brightness()?,
            pyfmt::repr_f64(start.elapsed().as_secs_f64())
        );
        Ok(())
    }
}

fn read_int(path: &std::path::Path) -> Result<i64, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    pyfmt::py_int(text.trim()).ok_or_else(|| format!("invalid literal for int(): {text:?}"))
}

fn python_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
        Value::Bool(_) => "bool",
        Value::Number(_) => "float",
    }
}

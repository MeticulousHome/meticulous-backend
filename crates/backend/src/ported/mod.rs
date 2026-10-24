//! Ports of the Python suite's handler tests (`tests/test_*.py`) that test
//! the backend through its HTTP routes, Socket.IO events and files. Each
//! module is named after the Python file it ports, and each test names the
//! Python case in its doc comment.

mod test_actions;
mod test_api_base_handler;
mod test_api_machine;
mod test_api_notifications;
mod test_api_profiles;
mod test_api_settings;
mod test_api_sounds;
mod test_config;
mod test_local_access_policy;
mod test_machine_device_uuid;
mod test_manual_mode;
mod test_pour_over;
mod test_settings_validation;
mod test_shot_debug_log_lock;
mod test_shot_manager;

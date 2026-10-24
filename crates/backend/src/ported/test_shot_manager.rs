//! tests/test_shot_manager.py: the push-to-brew wait a shot records. The
//! timer's own cases are in `shots.rs`.

use serde_json::json;

use crate::shots::{PUSH_TO_BREW_STATUS, PushToBrewTimer, Shot};
use crate::testing::{app, call, json as body};

/// tests/test_shot_manager.py::test_normal_shot_serializes_push_to_brew_time
#[test]
fn normal_shot_serializes_push_to_brew_time() {
    let shot = Shot::new(987);

    assert_eq!(shot.to_json()["push_to_brew_time"], json!(987));
}

/// tests/test_shot_manager.py::test_normal_shot_without_push_to_brew_stage_serializes_zero:
/// a brew that never waited on "click to start" starts its shot with the
/// timer's zero.
#[test]
fn normal_shot_without_push_to_brew_stage_serializes_zero() {
    let mut timer = PushToBrewTimer::default();
    for (status, at) in [("heating", 1.0), ("closing valve", 2.0), ("Infusion", 3.0)] {
        assert_ne!(status, PUSH_TO_BREW_STATUS);
        timer.observe(status, at);
    }

    let shot = Shot::new(timer.duration_ms);

    assert_eq!(shot.to_json()["push_to_brew_time"], json!(0));
}

/// tests/test_shot_manager.py::test_shot_manager_starts_normal_shot_with_push_to_brew_time
#[tokio::test]
async fn shot_manager_starts_normal_shot_with_push_to_brew_time() {
    let (_dir, app) = app();

    app.shots.start(654);

    assert_eq!(app.shots.current_shot()["push_to_brew_time"], json!(654));
    let current = call(&app, http::Method::GET, "/api/v1/history/current", "").await;
    assert_eq!(body(&current)["push_to_brew_time"], json!(654));
}

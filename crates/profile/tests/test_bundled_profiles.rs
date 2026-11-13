//! `tests/test_bundled_profiles.py` and the pure part of
//! `tests/test_simple_profile.py`: the profiles the backend ships pass the
//! validation it applies to every profile, and the bundled schema copy is
//! the one the submodule pins.

mod common;

use common::{repo_file, repo_json};
use profile::pour_over::pour_over_validator;
use profile::profiles::{ESPRESSO_PROFILE_SCHEMA, espresso_validator};
use profile::validate_profile;
use serde_json::{Value, json};

/// `test_bundled_profile_passes_the_backend_validation[simple_profile.json]`.
#[test]
fn simple_profile_passes_the_backend_validation() {
    let profile = repo_json("simple_profile.json");
    assert_eq!(validate_profile(&profile), Ok(()));
}

/// `test_bundled_profile_passes_the_backend_validation[example_profile.json]`.
#[test]
fn schema_example_passes_the_backend_validation() {
    let profile = repo_json("profile_schema/example_profile.json");
    assert_eq!(validate_profile(&profile), Ok(()));
}

/// `test_the_default_profile_served_is_the_bundled_one`: the served copy is
/// the file itself (loading it is the backend's), and it validates.
#[test]
fn the_default_profile_validates_as_served() {
    let served = repo_json("simple_profile.json");
    assert_eq!(validate_profile(&served.clone()), Ok(()));
    assert_eq!(served, repo_json("simple_profile.json"));
}

/// `test_simple_profile.py::test_bundled_profile_is_the_dial_demo`.
#[test]
fn bundled_profile_is_the_dial_demo() {
    let profile = repo_json("simple_profile.json");
    assert_eq!(profile["id"], json!("f5db85cc-ab0d-4c81-aee8-bcac7da77141"));
    assert_eq!(profile["name"], json!("Simple"));
    assert_eq!(profile["temperature"], json!(90));
    assert_eq!(profile["final_weight"], json!(36));
    assert_eq!(profile["stages"].as_array().map(Vec::len), Some(2));
    assert!(
        profile["display"]["image"]
            .as_str()
            .expect("image")
            .ends_with("59894b10354b95395b434b49399966f2.png")
    );
}

/// The vendored schema is the submodule's `schema.json` (when the submodule
/// is checked out; the crate builds without it).
#[test]
fn vendored_schema_matches_the_submodule() {
    let path = repo_file("profile_schema/schema.json");
    match std::fs::read_to_string(&path) {
        Ok(submodule) => {
            let submodule: Value = serde_json::from_str(&submodule).expect("submodule schema");
            let vendored: Value =
                serde_json::from_str(ESPRESSO_PROFILE_SCHEMA).expect("vendored schema");
            assert_eq!(
                pyfmt::dumps_default(&vendored),
                pyfmt::dumps_default(&submodule),
                "crates/profile/schemas/espresso_profile_schema.json differs from \
                 profile_schema/schema.json: copy it over and regenerate the goldens"
            );
        }
        Err(_) => {
            eprintln!("profile_schema submodule not checked out; vendored schema not compared")
        }
    }
}

/// The validator implements every keyword the bundled schemas use.
#[test]
fn bundled_schemas_use_only_supported_keywords() {
    assert_eq!(
        espresso_validator().unsupported_keywords(),
        Vec::<String>::new()
    );
    assert_eq!(
        pour_over_validator().unsupported_keywords(),
        Vec::<String>::new()
    );
}

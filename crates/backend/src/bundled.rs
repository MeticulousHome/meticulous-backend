//! The package's small read-only data files, built into the binary.
//!
//! The .deb still installs each of them under `/opt/meticulous-backend` and
//! the file on disk wins: the machine image, a developer (`SIMPLE_PROFILE`,
//! `SYSTEM_SOUNDS`, `DEFAULT_IMAGES`) or a later package may put its own
//! there. The copy built in here is what a reader falls back to when the
//! file is missing, so a package that leaves one out (as a Python .deb once
//! left out `profile_converter/`) serves the same bytes instead of breaking.
//! A file that is there but unreadable or invalid keeps its reader's error
//! handling: it is somebody's override and its error is worth seeing.
//!
//! Not built in, on purpose:
//! - the sound files: `gst-play-1.0` plays a path, so they ship as files and
//!   only the default theme's `config.json` is here;
//! - the default profile images: 4.6 MB of PNG for a failure that is already
//!   loud, see `ProfileManager::refresh_image_list`;
//! - `UI_timezones.json`: the backend never reads the packaged copy, it
//!   generates its own (`TIMEZONE_JSON_FILE_PATH`) from the image's tzdata.

/// `simple_profile.json`, read by [`crate::simple_profile::SimpleProfile`].
pub const SIMPLE_PROFILE: &str = include_str!("../../../simple_profile.json");

/// `sounds/default/config.json`: the default sound theme's event to file
/// map, read by [`crate::sounds::SoundPlayer`].
pub const DEFAULT_SOUND_THEME: &str = include_str!("../../../sounds/default/config.json");

/// `images/default/accent_colors.json`: the accent color of each default
/// profile image by its md5 file name, read by
/// [`crate::profiles::ProfileManager`].
pub const DEFAULT_ACCENT_COLORS: &str = include_str!("../../../images/default/accent_colors.json");

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use http::Method;
    use serde_json::{Value, json};

    use crate::app::App;
    use crate::config::keys;
    use crate::env::BackendMode;
    use crate::testing::{app_custom, call, json};

    struct PackageApp {
        _package: tempfile::TempDir,
        _dir: tempfile::TempDir,
        app: Arc<App>,
    }

    /// An app booted (profiles, sounds) on the repository's data, the full
    /// package, or on a package root that has the default images and none of
    /// the files built in here.
    fn booted(stripped: bool) -> PackageApp {
        let package = tempfile::tempdir().expect("package root");
        let root = package.path().to_path_buf();
        let (dir, app) = app_custom(BackendMode::Emulator, |env| {
            if !stripped {
                return;
            }
            let images = root.join("images/default");
            std::fs::create_dir_all(&images).expect("images folder");
            for entry in std::fs::read_dir(&env.paths.default_images).expect("repository images") {
                let path = entry.expect("entry").path();
                if path.extension().is_some_and(|e| e == "png") {
                    std::fs::copy(&path, images.join(path.file_name().expect("name")))
                        .expect("copy image");
                }
            }
            env.paths.default_images = images;
            env.paths.system_sounds = root.join("sounds");
            env.paths.simple_profile = root.join("simple_profile.json");
            env.paths.package_root = root.clone();
        });
        app.profiles.init(&app);
        app.sounds.init(true, false);
        PackageApp {
            _package: package,
            _dir: dir,
            app,
        }
    }

    async fn assert_same_reply(full: &Arc<App>, stripped: &Arc<App>, method: Method, uri: &str) {
        let expected = call(full, method.clone(), uri, "").await;
        let reply = call(stripped, method, uri, "").await;
        assert_eq!(reply.status_code(), expected.status_code(), "{uri}");
        assert_eq!(
            reply.header("Content-Type"),
            expected.header("Content-Type"),
            "{uri}"
        );
        assert_eq!(
            String::from_utf8_lossy(reply.body()),
            String::from_utf8_lossy(expected.body()),
            "{uri}"
        );
    }

    /// A package without simple_profile.json, the sounds folder and
    /// accent_colors.json serves what the full package serves: the simple
    /// profile, the default sound theme, and the accent color a profile
    /// saved with a bundled image and none of its own receives.
    #[tokio::test]
    async fn package_without_the_files_serves_the_same_bytes() {
        let full = booted(false);
        let stripped = booted(true);
        let (full, stripped) = (&full.app, &stripped.app);
        assert!(!stripped.env.paths.simple_profile.exists());
        assert!(!stripped.env.paths.system_sounds.exists());
        assert!(
            !stripped
                .env
                .paths
                .default_images
                .join("accent_colors.json")
                .exists()
        );

        for uri in [
            "/api/v1/profile/defaults",
            "/api/v1/profile/image",
            "/api/v1/sounds/list",
            "/api/v1/sounds/theme/list",
            "/api/v1/sounds/theme/get",
            "/api/v1/sounds/theme/set/default",
        ] {
            assert_same_reply(full, stripped, Method::GET, uri).await;
        }

        // A factory machine offers only the simple profile.
        for app in [full, stripped] {
            app.config
                .set(keys::USER, keys::UPDATE_CHANNEL, json!("factory"));
        }
        let simple: Value = serde_json::from_str(super::SIMPLE_PROFILE).expect("JSON");
        let simple_id = simple["id"].as_str().expect("id");
        for uri in [
            "/api/v1/profile/list".to_string(),
            "/api/v1/profile/list?full=true".to_string(),
            format!("/api/v1/profile/get/{simple_id}"),
        ] {
            assert_same_reply(full, stripped, Method::GET, &uri).await;
        }
        for app in [full, stripped] {
            app.config
                .set(keys::USER, keys::UPDATE_CHANNEL, json!("stable"));
        }

        let id = "3b0e5d7a-8f43-4c1e-9b2a-6d5f4e3c2b1a";
        let mut profile = simple.clone();
        profile["id"] = json!(id);
        profile["display"]
            .as_object_mut()
            .expect("display")
            .shift_remove("accentColor");
        let mut displays = Vec::new();
        for app in [full, stripped] {
            let saved = call(
                app,
                Method::POST,
                "/api/v1/profile/save",
                profile.to_string(),
            )
            .await;
            assert_eq!(
                saved.status_code(),
                200,
                "{}",
                String::from_utf8_lossy(saved.body())
            );
            let reply = call(app, Method::GET, &format!("/api/v1/profile/get/{id}"), "").await;
            displays.push(json(&reply)["display"].clone());
        }
        let image = simple["display"]["image"].as_str().expect("image");
        let colors: Value = serde_json::from_str(super::DEFAULT_ACCENT_COLORS).expect("JSON");
        let color = &colors[image.rsplit('/').next().expect("file name")];
        assert!(color.is_string());
        assert_eq!(displays[0]["accentColor"], *color);
        assert_eq!(displays[1], displays[0]);
    }

    /// A user theme named like the default one is not replaced by the
    /// built-in config when the package has no sounds.
    #[test]
    fn built_in_sound_theme_does_not_replace_a_user_default_theme() {
        let stripped = booted(true);
        let app = &stripped.app;
        let user_default = app.env.paths.user_sounds.join("default");
        std::fs::create_dir_all(&user_default).expect("user theme");
        std::fs::write(
            user_default.join("config.json"),
            r#"{"startup": "mine.mp3"}"#,
        )
        .expect("user config");
        app.sounds.init(true, false);
        assert_eq!(app.sounds.available_themes(), ["default"]);
        assert_eq!(app.sounds.theme_config()["startup"], "mine.mp3");
    }

    /// Every built-in file parses the way its reader needs it to.
    #[test]
    fn built_in_files_are_json_objects() {
        for text in [
            super::SIMPLE_PROFILE,
            super::DEFAULT_SOUND_THEME,
            super::DEFAULT_ACCENT_COLORS,
        ] {
            let parsed: Value = serde_json::from_str(text).expect("JSON");
            assert!(parsed.is_object());
        }
        let profile: Value = serde_json::from_str(super::SIMPLE_PROFILE).expect("JSON");
        assert!(profile["id"].is_string());
    }
}

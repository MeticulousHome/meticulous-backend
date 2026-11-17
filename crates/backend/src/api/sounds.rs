//! `api/sounds.py`.

use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::app::App;
use crate::config::keys;
use crate::http::{Kind, Reply, Request, Route};

pub fn routes() -> Vec<Route> {
    let m = "api.sounds";
    vec![
        Route::new("/sounds/play/(.*)", Kind::Base, m, "PlaySoundHandler").get(play),
        Route::new("/sounds/list", Kind::Base, m, "ListSoundsHandler").get(list_sounds),
        Route::new("/sounds/theme/list", Kind::Base, m, "ListThemesHandler").get(list_themes),
        Route::new("/sounds/theme/get", Kind::Base, m, "GetThemeHandler").get(get_theme),
        Route::new("/sounds/theme/set/(.*)", Kind::Base, m, "SetThemeHandler")
            .get(set_theme)
            .post(set_theme),
        Route::new("/sounds/theme/upload", Kind::Base, m, "UploadThemeHandler").post(upload),
    ]
}

async fn play(app: Arc<App>, req: Request) -> Reply {
    let sound = req.arg(0).unwrap_or("").to_string();
    let played = {
        let app = app.clone();
        let sound = sound.clone();
        tokio::task::spawn_blocking(move || app.sounds.play(&sound))
            .await
            .unwrap_or(false)
    };
    if played {
        Reply::base().json(json!({"status": "okay"}))
    } else {
        Reply::base()
            .status(404)
            .json(json!({"error": "sound not found", "details": sound}))
    }
}

async fn list_sounds(app: Arc<App>, _req: Request) -> Reply {
    let keys: Vec<Value> = app
        .sounds
        .theme_config()
        .keys()
        .map(|k| Value::String(k.clone()))
        .collect();
    Reply::base().dumps(Value::Array(keys))
}

async fn list_themes(app: Arc<App>, _req: Request) -> Reply {
    let themes: Vec<Value> = app
        .sounds
        .available_themes()
        .into_iter()
        .map(Value::String)
        .collect();
    Reply::base().dumps(Value::Array(themes))
}

async fn get_theme(app: Arc<App>, req: Request) -> Reply {
    match app.config.get(keys::SYSTEM, keys::SOUNDS_THEME) {
        Value::String(theme) => {
            let mut reply = Reply::base();
            reply.write_str(&theme);
            reply
        }
        // PARITY: tornado's write(None) raises, so a cleared theme is a 500.
        _ => Reply::error(Kind::Base, 500, &req),
    }
}

async fn set_theme(app: Arc<App>, req: Request) -> Reply {
    let theme = req.arg(0).unwrap_or("").to_string();
    let set = {
        let app = app.clone();
        let theme = theme.clone();
        tokio::task::spawn_blocking(move || app.sounds.set_theme(Some(&theme)))
            .await
            .unwrap_or(false)
    };
    if set {
        Reply::base().json(json!({"status": "okay"}))
    } else {
        Reply::base()
            .status(404)
            .json(json!({"error": "theme not found", "details": theme}))
    }
}

fn invalid_zip(details: &str) -> Reply {
    Reply::base()
        .status(400)
        .json(json!({"error": "invalid zip", "details": details}))
}

async fn upload(app: Arc<App>, req: Request) -> Reply {
    let files = req.files().await;
    let Some(file) = files.into_iter().find(|f| f.field == "file") else {
        return invalid_zip("'file' not found in request");
    };
    let result = tokio::task::spawn_blocking(move || install_theme(&app, &file.body)).await;
    match result {
        Ok(Ok(())) => {
            let mut reply = Reply::base();
            reply.write_str("Zip file uploaded and unpacked successfully.");
            reply
        }
        Ok(Err(reply)) => reply,
        Err(error) => Reply::base()
            .status(500)
            .json(json!({"error": "unknown issue", "details": error.to_string()})),
    }
}

fn install_theme(app: &App, bytes: &[u8]) -> Result<(), Reply> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid_zip("zip file corrupted"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.to_string()))
        .collect();
    let roots: BTreeSet<&str> = names
        .iter()
        .filter(|n| n.contains('/'))
        .filter_map(|n| n.split('/').next())
        .collect();
    if roots.len() != 1 {
        return Err(invalid_zip(
            "Zip must contain exactly one folder with the themes name at the root.",
        ));
    }
    let root = roots.into_iter().next().unwrap_or_default().to_string();
    if !names.contains(&format!("{root}/config.json")) {
        return Err(invalid_zip("No config.json found in the root folder."));
    }
    for name in names.iter().filter(|n| n.ends_with("config.json")) {
        let mut text = Vec::new();
        archive
            .by_name(name)
            .map_err(|_| invalid_zip("zip file corrupted"))?
            .read_to_end(&mut text)
            .map_err(|_| invalid_zip("zip file corrupted"))?;
        if serde_json::from_slice::<Value>(&text).is_err() {
            return Err(invalid_zip("config.json is not valid JSON."));
        }
    }
    let target = &app.env.paths.user_sounds;
    tracing::info!("Extracting zip to {}", target.display());
    archive.extract(target).map_err(|e| {
        Reply::base()
            .status(500)
            .json(json!({"error": "unknown issue", "details": e.to_string()}))
    })?;
    tracing::info!("Reloading sound player");
    app.sounds.init(app.emulated(), false);
    Ok(())
}

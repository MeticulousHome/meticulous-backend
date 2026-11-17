//! `api/update.py`: upload a firmware for the ESP32 and flash it.

use std::io::Cursor;
use std::sync::Arc;

use crate::app::App;
use crate::http::{Kind, Reply, Request, Route};

const IMAGE_NAMES: [&str; 4] = [
    "firmware.bin",
    "partitions.bin",
    "bootloader.bin",
    "boot_app0.bin",
];

pub fn routes() -> Vec<Route> {
    vec![
        Route::new(
            "/update/firmware",
            Kind::Base,
            "api.update",
            "UpdateFirmwareWithZipHandler",
        )
        .post(upload),
    ]
}

fn text(status: u16, body: &str) -> Reply {
    let mut reply = Reply::base().status(status);
    reply.write_str(body);
    reply
}

async fn upload(app: Arc<App>, req: Request) -> Reply {
    let Some(chip) = req.get_argument("chip").filter(|c| !c.is_empty()) else {
        return text(400, "Missing 'chip' parameter");
    };
    tracing::info!("Flash request for an {chip}");
    let Some(chip) = crate::esp_tool::FikaSupportedEsp32::from_string(&chip) else {
        return text(
            400,
            &format!(
                "Invalid 'chip' parameter. Allowed (case-insensitive): {}",
                "['ESP32S1', 'ESP32S3']"
            ),
        );
    };
    let files: Vec<_> = req
        .files()
        .await
        .into_iter()
        .filter(|f| f.field == "file")
        .collect();
    if files.is_empty() {
        return text(400, "No file uploaded.");
    }
    let target = crate::esp_tool::firmware_path(&app.env.paths.firmware, chip);
    let mut reply = Reply::base();
    let mut failed = false;
    for file in files {
        if !file.filename.ends_with(".zip") {
            if !IMAGE_NAMES.contains(&file.filename.as_str()) {
                return text(
                    400,
                    "Invalid file format. Only ZIP files and certain images are accepted.",
                );
            }
            let path = target.join(&file.filename);
            match std::fs::create_dir_all(&target).and_then(|_| std::fs::write(&path, &file.body)) {
                Ok(()) => tracing::info!("File uploaded to {}", path.display()),
                Err(error) => {
                    reply.set_status(400);
                    reply.write_str(&format!("An error occurred during file upload: {error}"));
                    failed = true;
                }
            }
        } else {
            match zip::ZipArchive::new(Cursor::new(file.body.as_ref())) {
                Err(_) => {
                    reply.set_status(400);
                    reply.write_str("The uploaded file is not a valid ZIP archive.");
                    failed = true;
                }
                Ok(mut archive) => match std::fs::create_dir_all(&target)
                    .map_err(|e| e.to_string())
                    .and_then(|_| archive.extract(&target).map_err(|e| e.to_string()))
                {
                    Ok(()) => tracing::info!("File unpacked to {}", target.display()),
                    Err(error) => {
                        reply.set_status(400);
                        reply.write_str(&format!("An error occurred during zip upload: {error}"));
                        failed = true;
                    }
                },
            }
        }
    }
    if failed {
        reply.write_str("failure during upload");
        return reply;
    }
    let flash_app = app.clone();
    tokio::task::spawn_blocking(move || {
        flash_app.machine.refresh_available_firmware(&flash_app);
        let _ = std::thread::Builder::new()
            .name("FWUpgrade".into())
            .spawn(move || {
                flash_app.machine.start_update(&flash_app);
            });
    })
    .await
    .ok();
    reply.write_str("success");
    reply
}

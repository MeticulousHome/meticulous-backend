//! `api/serial.py`: the factory form that writes the serial number to the
//! ESP32, and the scale calibration trigger.

use std::sync::Arc;

use crate::app::App;
use crate::http::{Kind, Reply, Request, Route};

const WEBPAGE: &str = r#"
<!DOCTYPE html>
<html>
<head>
    <title>Product Form</title>
</head>
<body>
    <form action="serial" method="post">
        <label for="color">Color:</label>
        <input type="text" id="color" name="color" value="black"><br><br>

        <label for="serial">Serial:</label>
        <input type="text" id="serial" name="serial"><br><br>

        <label for="batch_number">Batch Number:</label>
        <input type="text" id="batch_number" name="batch_number"><br><br>

        <label for="build_date">Build Date:</label>
        <input type="date" id="build_date" name="build_date"><br><br>

        <input type="submit" value="Submit">
    </form>

    <script>
        // Set the build date to today's date
        document.addEventListener('DOMContentLoaded', () => {
            const today = new Date().toISOString().split('T')[0];
            document.getElementById('build_date').value = today;
        });
    </script>
</body>
</html>
"#;

pub fn routes() -> Vec<Route> {
    let m = "api.serial";
    vec![
        Route::new("/serial", Kind::Base, m, "SerialNumberHandler")
            .get(form)
            .post(submit),
        Route::new("/scaleCalibrate", Kind::Base, m, "ScaleCalibrateHandler").get(calibrate),
    ]
}

async fn form(_app: Arc<App>, _req: Request) -> Reply {
    let mut reply = Reply::base();
    reply.set_header("Content-Type", "text/html");
    reply.write_str(WEBPAGE);
    reply
}

async fn submit(app: Arc<App>, req: Request) -> Reply {
    let mut reply = Reply::base();
    reply.set_header("Content-Type", "text/html");
    let color = req
        .get_body_argument("color")
        .unwrap_or_else(|| "black".into());
    // tornado answers 400 when a required body argument is missing.
    let (Some(serial), Some(batch_number), Some(build_date)) = (
        req.get_body_argument("serial"),
        req.get_body_argument("batch_number"),
        req.get_body_argument("build_date"),
    ) else {
        return Reply::error(Kind::Base, 400, &req);
    };
    for (value, message) in [
        (&color, "Color is required"),
        (&serial, "Serial is required"),
        (&batch_number, "Batch number is required"),
        (&build_date, "Build date is required"),
    ] {
        if value.is_empty() {
            reply.write_str(message);
            return reply;
        }
    }
    {
        let app = app.clone();
        let (color, serial, batch_number, build_date) = (
            color.clone(),
            serial.clone(),
            batch_number.clone(),
            build_date.clone(),
        );
        let _ = tokio::task::spawn_blocking(move || {
            app.machine
                .set_serial(&app, &color, &serial, &batch_number, &build_date)
        })
        .await;
    }
    reply.write_str(&format!(
        "Received Data:<br>Color: {color}<br>Serial: {serial}<br>Batch Number: {batch_number}<br>Build Date: {build_date}"
    ));
    reply
}

async fn calibrate(app: Arc<App>, _req: Request) -> Reply {
    let _ =
        tokio::task::spawn_blocking(move || app.machine.start_scale_master_calibration(&app)).await;
    Reply::base()
}

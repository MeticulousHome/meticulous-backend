//! `api/web_ui.py`: redirects to the debug UI and its static files.

use crate::app::App;
use crate::http::{Kind, Route, StaticOptions};

pub fn routes(app: &App) -> Vec<Route> {
    let m = "api.web_ui";
    vec![
        Route::new("", Kind::Redirect("/".into()), m, "RedirectHandler"),
        Route::new(
            "/graph",
            Kind::Redirect("/debug/graph.html".into()),
            m,
            "RedirectHandler",
        ),
        Route::new(
            "/debug",
            Kind::Redirect("/debug/graph.html".into()),
            m,
            "RedirectHandler",
        ),
        Route::new(
            "/debug/(.*)",
            Kind::Static(StaticOptions {
                root: app.env.paths.package_root.join("api/web_ui"),
                default_filename: Some("graph.html".into()),
            }),
            m,
            "StaticFileHandler",
        ),
    ]
}

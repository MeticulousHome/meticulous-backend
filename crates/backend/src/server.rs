//! The HTTP server: tornado's routes behind a Socket.IO layer, on
//! `127.0.0.1:PORT` (nginx is the only way in from the network).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use serde_json::Value;
use socketioxide::SocketIo;
use socketioxide::extract::{SocketRef, TryData};

use crate::app::App;
use crate::http::Router;

/// Bind and serve until the process ends. `on_listening` runs once the
/// socket is bound, like the code after `app.listen()` in `backend.main()`.
pub async fn serve(
    app: Arc<App>,
    router: Arc<Router>,
    on_listening: impl FnOnce(),
) -> std::io::Result<()> {
    let (layer, io) = SocketIo::builder()
        // python-socketio AsyncServer(ping_interval=10, ping_timeout=60)
        .ping_interval(Duration::from_secs(10))
        .ping_timeout(Duration::from_secs(60))
        .build_layer();
    register_socket_handlers(&io, app.clone());
    app.sio.attach(io);

    let fallback_app = app.clone();
    let service = axum::Router::new()
        .fallback(
            move |ConnectInfo(remote): ConnectInfo<SocketAddr>, request: axum::extract::Request| {
                let router = router.clone();
                let app = fallback_app.clone();
                async move { crate::http::serve(router, app, Some(remote), request).await }
            },
        )
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(layer)
        .layer(axum::middleware::from_fn(socketio_cors));

    let address = SocketAddr::from(([127, 0, 0, 1], app.env.port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!("Listening on {address}");
    on_listening();
    axum::serve(
        listener,
        service.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

/// python-engineio's CORS handling with `cors_allowed_origins="*"`: the
/// Socket.IO endpoint mirrors the request's origin and allows credentials,
/// and answers preflights itself. The API routes keep tornado's headers.
async fn socketio_cors(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if !request.uri().path().starts_with("/socket.io") {
        return next.run(request).await;
    }
    let origin = request.headers().get(http::header::ORIGIN).cloned();
    let requested_headers = request
        .headers()
        .get(http::header::ACCESS_CONTROL_REQUEST_HEADERS)
        .cloned();
    let mut response = if request.method() == http::Method::OPTIONS {
        let mut preflight = axum::response::Response::new(axum::body::Body::empty());
        let headers = preflight.headers_mut();
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_METHODS,
            http::HeaderValue::from_static("OPTIONS, GET, POST"),
        );
        if let Some(requested) = requested_headers {
            headers.insert(http::header::ACCESS_CONTROL_ALLOW_HEADERS, requested);
        }
        preflight
    } else {
        next.run(request).await
    };
    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            http::HeaderValue::from_static("true"),
        );
    }
    response
}

/// The Socket.IO events the Python's `backend.py` handles.
fn register_socket_handlers(io: &SocketIo, app: Arc<App>) {
    io.ns("/", move |socket: SocketRef| {
        let app = app.clone();
        async move { on_socket(app, socket) }
    });
}

fn on_socket(app: Arc<App>, socket: SocketRef) {
    tracing::info!("connect {}", socket.id);
    crate::events::on_connect(&app, &socket.id.to_string());

    let a = app.clone();
    socket.on("action", move |TryData::<Value>(data)| {
        let app = a.clone();
        async move { crate::events::on_action(&app, data.ok()) }
    });
    let a = app.clone();
    socket.on("notification", move |TryData::<Value>(data)| {
        let app = a.clone();
        async move { crate::events::on_notification(&app, data.ok()) }
    });
    let a = app.clone();
    socket.on(
        "profileHover",
        move |s: SocketRef, TryData::<Value>(data)| {
            let app = a.clone();
            async move { crate::events::on_profile_hover(&app, &s.id.to_string(), data.ok()) }
        },
    );
    let a = app.clone();
    socket.on("calibrate", move || {
        let app = a.clone();
        async move { crate::events::on_calibrate(&app) }
    });
    socket.on_disconnect(async |s: SocketRef| {
        tracing::info!("disconnect {}", s.id);
    });
}

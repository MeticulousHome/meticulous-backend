//! What tornado 6.5 does with a request, as the Dial, the app and nginx see it.
//!
//! Routes are regexes tried in order, first full match wins, exactly like
//! `tornado.web.Application`; handlers build a [`Reply`] the way a tornado
//! `RequestHandler` builds its response (default headers, `write(dict)`,
//! automatic `Etag` and `304`, no representation headers on `204`), and the
//! error pages are tornado's.

mod reply;
mod request;
mod static_files;

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use regex::Regex;

pub use reply::{Kind, Reply, http_date};
pub use request::{Request, parse_python_json};
pub use static_files::{StaticOptions, absolute, serve_file, serve_static};

use crate::app::App;

pub type HandlerFuture = Pin<Box<dyn Future<Output = Reply> + Send>>;
pub type HandlerFn = Arc<dyn Fn(Arc<App>, Request) -> HandlerFuture + Send + Sync>;
/// `RequestHandler.prepare`: a reply finishes the request.
pub type Prepare = fn(&Request) -> Option<Reply>;
pub type GuardFuture = Pin<Box<dyn Future<Output = Option<Reply>> + Send>>;
/// A check every route of a [`Router`] runs after its `prepare`, unless the
/// route is public for the method: a reply finishes the request.
pub type Guard = fn(Arc<App>, Request) -> GuardFuture;

/// HTTP methods tornado dispatches to handler methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Head,
    Post,
    Delete,
    Patch,
    Put,
    Options,
}

impl Method {
    pub fn parse(method: &http::Method) -> Option<Method> {
        Some(match *method {
            http::Method::GET => Method::Get,
            http::Method::HEAD => Method::Head,
            http::Method::POST => Method::Post,
            http::Method::DELETE => Method::Delete,
            http::Method::PATCH => Method::Patch,
            http::Method::PUT => Method::Put,
            http::Method::OPTIONS => Method::Options,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Put => "PUT",
            Method::Options => "OPTIONS",
        }
    }
}

/// One tornado `URLSpec`: a pattern, the kind of handler class it maps to,
/// and the methods that class implements.
pub struct Route {
    pub pattern: String,
    pub regex: Regex,
    pub kind: Kind,
    /// The Python handler class name, for logs and the contract snapshot.
    pub handler: &'static str,
    /// The Python module the handler lives in, for the contract snapshot.
    pub module: &'static str,
    pub methods: Vec<(Method, HandlerFn)>,
    /// The handler class's `prepare()`, when it has one that can answer.
    pub prepare: Option<Prepare>,
    /// The methods the Python handler class defines, when they differ from
    /// what the route serves (see [`Route::contract_methods`]).
    class_methods: Option<Vec<&'static str>>,
    /// The methods the router's guard lets through.
    public: Vec<Method>,
}

impl Route {
    pub fn new(pattern: &str, kind: Kind, module: &'static str, handler: &'static str) -> Route {
        // tornado anchors the end of every pattern and matches from the start.
        let anchored = if pattern.ends_with('$') {
            format!("^{pattern}")
        } else {
            format!("^{pattern}$")
        };
        Route {
            pattern: pattern.to_string(),
            regex: Regex::new(&anchored).unwrap_or_else(|e| panic!("bad route {pattern}: {e}")),
            kind,
            handler,
            module,
            methods: Vec::new(),
            prepare: None,
            class_methods: None,
            public: Vec::new(),
        }
    }

    /// Serve these methods without the router's guard.
    pub fn public(mut self, methods: &[Method]) -> Route {
        self.public.extend_from_slice(methods);
        self
    }

    pub fn is_public(&self, method: Method) -> bool {
        self.public.contains(&method)
    }

    /// Set the handler's `prepare()`. tornado runs it for every method it
    /// supports before looking for the handler's own, so its reply also
    /// answers a preflight and a method the class does not define.
    pub fn prepare(mut self, prepare: Prepare) -> Route {
        self.prepare = Some(prepare);
        self
    }

    /// Add a handler method.
    pub fn on<F, Fut>(mut self, method: Method, f: F) -> Route
    where
        F: Fn(Arc<App>, Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        let f: HandlerFn = Arc::new(move |app, req| Box::pin(f(app, req)));
        self.methods.push((method, f));
        self
    }

    pub fn get<F, Fut>(self, f: F) -> Route
    where
        F: Fn(Arc<App>, Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        self.on(Method::Get, f)
    }

    pub fn post<F, Fut>(self, f: F) -> Route
    where
        F: Fn(Arc<App>, Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        self.on(Method::Post, f)
    }

    pub fn put<F, Fut>(self, f: F) -> Route
    where
        F: Fn(Arc<App>, Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        self.on(Method::Put, f)
    }

    pub fn delete<F, Fut>(self, f: F) -> Route
    where
        F: Fn(Arc<App>, Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        self.on(Method::Delete, f)
    }

    fn handler_for(&self, method: Method) -> Option<&HandlerFn> {
        self.methods
            .iter()
            .find(|(m, _)| *m == method)
            .map(|(_, f)| f)
    }

    pub fn method_names(&self) -> Vec<&'static str> {
        self.methods.iter().map(|(m, _)| m.as_str()).collect()
    }

    /// Record the methods the Python handler class defines.
    pub fn class_methods(mut self, methods: &[&'static str]) -> Route {
        self.class_methods = Some(methods.to_vec());
        self
    }

    /// The methods the handler class defines, as `extract.py` lists them:
    /// tornado's redirect and static file handlers serve `get`.
    pub fn contract_methods(&self) -> Vec<String> {
        if let Some(methods) = &self.class_methods {
            return methods.iter().map(|m| m.to_string()).collect();
        }
        match self.kind {
            Kind::Redirect(_) | Kind::Static(_) => vec!["get".to_string()],
            _ => self
                .method_names()
                .into_iter()
                .map(str::to_ascii_lowercase)
                .collect(),
        }
    }
}

/// The route table in the order tornado tries it.
pub struct Router {
    routes: Vec<Route>,
    guard: Option<Guard>,
}

impl Router {
    pub fn new(routes: Vec<Route>) -> Router {
        Router {
            routes,
            guard: None,
        }
    }

    /// Run `guard` on every request a non-public route serves, after the
    /// route's `prepare` (so its refusals stay first) and before static
    /// files and redirects.
    pub fn guarded(mut self, guard: Guard) -> Router {
        self.guard = Some(guard);
        self
    }

    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// Serve one request: route, call the handler method, log like
    /// `tornado.access`.
    pub async fn dispatch(&self, app: Arc<App>, mut req: Request) -> Reply {
        let started = Instant::now();
        let reply = self.route(app, &mut req).await;
        log_access(&req, &reply, started);
        reply
    }

    async fn route(&self, app: Arc<App>, req: &mut Request) -> Reply {
        let Some(route) = self.routes.iter().find(|r| r.regex.is_match(&req.path)) else {
            // tornado's ErrorHandler is a plain RequestHandler.
            return Reply::error(Kind::Plain, 404, req);
        };
        if let Some(caps) = route.regex.captures(&req.path) {
            req.path_args = caps
                .iter()
                .skip(1)
                .map(|m| m.map(|m| pyfmt::unquote(m.as_str())))
                .collect();
        }
        let Some(method) = Method::parse(&req.method) else {
            return Reply::error(route.kind.clone(), 405, req);
        };
        if let Some(prepare) = route.prepare
            && let Some(reply) = prepare(req)
        {
            return reply.finish(req);
        }
        if let Some(guard) = self.guard
            && !route.is_public(method)
            && let Some(reply) = guard(app.clone(), req.clone()).await
        {
            return reply.finish(req);
        }
        match &route.kind {
            Kind::Static(options) if matches!(method, Method::Get | Method::Head) => {
                return serve_static(options, req).await;
            }
            Kind::Redirect(url) if method == Method::Get => {
                return Reply::redirect(url, req);
            }
            _ => {}
        }
        match route.handler_for(method) {
            Some(handler) => {
                let reply = handler(app, req.clone()).await;
                reply.finish(req)
            }
            // BaseHandler answers every OPTIONS preflight with an empty 204.
            None if method == Method::Options && route.kind == Kind::Base => {
                Reply::new(Kind::Base).status(204).finish(req)
            }
            None => Reply::error(route.kind.clone(), 405, req),
        }
    }
}

/// Paths whose last segment is a secret: an access request's id lets
/// whoever holds it read the pairing code the Dial approved.
const SECRET_PATH_PREFIXES: [&str; 1] = ["/api/v1/auth/request/"];

/// The request's URI as the access log may show it.
fn loggable_uri(uri: &str) -> String {
    for prefix in SECRET_PATH_PREFIXES {
        if uri.len() > prefix.len() && uri.starts_with(prefix) {
            return format!("{prefix}<id>");
        }
    }
    uri.to_string()
}

fn log_access(req: &Request, reply: &Reply, started: Instant) {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let line = format!(
        "{} {} {} ({}) {:.2}ms",
        reply.status_code(),
        req.method,
        loggable_uri(&req.uri),
        req.remote_ip,
        ms
    );
    match reply.status_code() {
        s if s < 400 => tracing::info!(target: "tornado.access", "{line}"),
        s if s < 500 => tracing::warn!(target: "tornado.access", "{line}"),
        _ => tracing::error!(target: "tornado.access", "{line}"),
    }
}

/// Turn an axum request into ours and our reply into an axum response.
pub async fn serve(
    router: Arc<Router>,
    app: Arc<App>,
    remote: Option<SocketAddr>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let req = match Request::from_axum(request, remote).await {
        Ok(req) => req,
        Err(status) => {
            return axum::response::Response::builder()
                .status(status)
                .body(axum::body::Body::empty())
                .unwrap_or_default();
        }
    };
    router.dispatch(app, req).await.into_axum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An access request's id reads its approved pairing code, so the
    /// access log (kept on the machine, sent with bug reports) never shows
    /// it; other URIs are logged as sent.
    #[test]
    fn access_log_hides_access_request_ids() {
        assert_eq!(
            loggable_uri("/api/v1/auth/request/0b7c3a52-5d1e-4c5f-9f43-2a1d7e8c9b10"),
            "/api/v1/auth/request/<id>"
        );
        assert_eq!(loggable_uri("/api/v1/auth/request"), "/api/v1/auth/request");
        assert_eq!(
            loggable_uri("/api/v1/history?max_results=3"),
            "/api/v1/history?max_results=3"
        );
    }
}

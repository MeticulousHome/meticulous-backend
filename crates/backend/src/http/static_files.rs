//! `tornado.web.StaticFileHandler`: profile images and the debug web UI.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha512};

use super::reply::{Kind, Reply, http_date};
use super::request::Request;

#[derive(Debug, Clone, PartialEq)]
pub struct StaticOptions {
    pub root: PathBuf,
    pub default_filename: Option<String>,
}

pub async fn serve_static(opts: &StaticOptions, req: &Request) -> Reply {
    let opts = opts.clone();
    let req = req.clone();
    tokio::task::spawn_blocking(move || serve_blocking(&opts, &req))
        .await
        .unwrap_or_else(|_| {
            Reply::error(
                Kind::Plain,
                500,
                &Request::synthetic(http::Method::GET, "/", ""),
            )
        })
}

fn serve_blocking(opts: &StaticOptions, req: &Request) -> Reply {
    let relative = req.arg(0).unwrap_or("");
    let root = absolute(&opts.root);
    let mut path = absolute(&root.join(relative));
    if !path.starts_with(&root) {
        return Reply::error(Kind::Plain, 403, req);
    }
    if path.is_dir() {
        match &opts.default_filename {
            Some(name) => {
                if !req.path.ends_with('/') {
                    return Reply::new(Kind::Plain)
                        .status(301)
                        .with_header("Location", format!("{}/", req.path))
                        .finish(req);
                }
                path = path.join(name);
            }
            None => return Reply::error(Kind::Plain, 403, req),
        }
    }
    if !path.exists() {
        return Reply::error(Kind::Plain, 404, req);
    }
    if !path.is_file() {
        return Reply::error(Kind::Plain, 403, req);
    }
    serve_file(&path, req, Reply::new(Kind::Plain), true, None)
}

/// `StaticFileHandler.get` once the path is resolved to a file: `reply`
/// carries the handler class's default headers, and `with_etag` is false
/// where the handler's `compute_etag` returns `None`. `content_type`
/// replaces the type guessed from the extension.
pub fn serve_file(
    path: &Path,
    req: &Request,
    mut reply: Reply,
    with_etag: bool,
    content_type: Option<&str>,
) -> Reply {
    let include_body = req.method == http::Method::GET;
    let Ok(content) = std::fs::read(path) else {
        return Reply::error(Kind::Plain, 404, req);
    };
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from);

    reply.set_header("Accept-Ranges", "bytes");
    let etag = with_etag.then(|| format!("\"{}\"", hex::encode(Sha512::digest(&content))));
    match &etag {
        Some(etag) => reply.set_header("Etag", etag.clone()),
        None => reply.disable_etag(),
    }
    if let Some(modified) = modified {
        reply.set_header("Last-Modified", http_date(modified));
    }
    let content_type = content_type.unwrap_or_else(|| {
        mime_guess::from_path(path)
            .first_raw()
            .unwrap_or("application/octet-stream")
    });
    reply.set_header("Content-Type", content_type);
    if req.query_arguments().iter().any(|(k, _)| k == "v") {
        let in_a_year = chrono::Utc::now() + chrono::Duration::days(365);
        reply.set_header("Expires", http_date(in_a_year));
        reply.set_header("Cache-Control", "max-age=31536000");
    }

    if not_modified(req, etag.as_deref(), modified) {
        return reply.status(304).finish(req);
    }

    let size = content.len();
    let (start, end) = match req.header("Range").and_then(|r| parse_range(r, size)) {
        Some(Ok(range)) => range,
        Some(Err(())) => {
            reply.set_header("Content-Type", "text/plain");
            reply.set_header("Content-Range", format!("bytes */{size}"));
            return reply.status(416).finish(req);
        }
        None => (0, size),
    };
    if (start, end) != (0, size) {
        reply.set_status(206);
        reply.set_header(
            "Content-Range",
            format!("bytes {}-{}/{}", start, end - 1, size),
        );
    }
    reply.set_header("Content-Length", (end - start).to_string());
    if include_body {
        reply.write_bytes(&content[start..end]);
    }
    reply.finish(req)
}

/// `StaticFileHandler.should_return_304`.
fn not_modified(
    req: &Request,
    etag: Option<&str>,
    modified: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    if let Some(tags) = req.header("If-None-Match") {
        // check_etag_header: nothing matches a response without an Etag.
        let Some(etag) = etag else {
            return false;
        };
        let strip = |t: &str| t.trim().trim_start_matches("W/").to_string();
        return tags
            .split(',')
            .any(|t| t.trim() == "*" || strip(t) == strip(etag));
    }
    if let (Some(since), Some(modified)) = (req.header("If-Modified-Since"), modified)
        && let Ok(since) = chrono::DateTime::parse_from_rfc2822(since)
    {
        return since.timestamp() >= modified.timestamp();
    }
    false
}

/// `httputil._parse_request_range` for a single `bytes=` range: `None` when
/// absent or not understood (the whole file is sent), `Err` when unsatisfiable.
fn parse_range(header: &str, size: usize) -> Option<Result<(usize, usize), ()>> {
    let spec = header.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", "") => return None,
        ("", suffix) => {
            let n: usize = suffix.parse().ok()?;
            (size.saturating_sub(n), size)
        }
        (s, "") => (s.parse().ok()?, size),
        (s, e) => (
            s.parse().ok()?,
            e.parse::<usize>().ok()?.saturating_add(1).min(size),
        ),
    };
    if size == 0 || start >= size || start >= end {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

/// `os.path.abspath`: absolute and normalized without touching the disk.
fn absolute(path: &Path) -> PathBuf {
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().unwrap_or_default()
    };
    let mut out = PathBuf::new();
    for component in base.join(path).components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some(Ok((0, 10))));
        assert_eq!(parse_range("bytes=90-", 100), Some(Ok((90, 100))));
        assert_eq!(parse_range("bytes=-10", 100), Some(Ok((90, 100))));
        assert_eq!(parse_range("bytes=200-", 100), Some(Err(())));
        assert_eq!(parse_range("items=0-1", 100), None);
    }

    #[test]
    fn abspath_normalizes() {
        assert_eq!(
            absolute(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(absolute(Path::new("/a/../../etc")), PathBuf::from("/etc"));
    }
}

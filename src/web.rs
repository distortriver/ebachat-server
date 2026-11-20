//! The web client as Go's web server hands it out: `/static/*` from the client directory, `/static/plugins/*`
//! from the plugins' client directory, `/robots.txt`, and `root.html` for any other GET outside `/api/`.
//!
//! The client directory is `EBACHAT_CLIENT_DIR` (Go finds `client/` next to its working directory or binary).
//! Static files are compressed as Go's `gzhttp` wrapper does in the default `WebserverMode` "gzip", with the
//! compressed bytes kept in memory per file and modification time.

// Based on Mattermost server/channels/web/static.go and web.go, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use axum::extract::{Path, Request, State as AxState};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::app::State;

pub fn client_dir() -> PathBuf {
    PathBuf::from(std::env::var("EBACHAT_CLIENT_DIR").unwrap_or_else(|_| "client".into()))
}

/// Go's `http.NotFound` behind `notFoundNoCacheResponseWriter`.
fn not_found() -> Response {
    let mut r = (StatusCode::NOT_FOUND, "404 page not found\n").into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, public"));
    r
}

/// `path.Clean("/" + p)` as `http.FileServer` does: `.` and `..` resolved, never above the root.
fn clean(p: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out
}

/// Go's built-in extension table first (`mime.builtinTypesLower`), then the wider guess.
fn content_type(name: &str) -> String {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let builtin = match ext.as_str() {
        "avif" => "image/avif",
        "css" => "text/css; charset=utf-8",
        "gif" => "image/gif",
        "htm" | "html" => "text/html; charset=utf-8",
        "jpeg" | "jpg" => "image/jpeg",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        "webp" => "image/webp",
        "xml" => "text/xml; charset=utf-8",
        _ => "",
    };
    if !builtin.is_empty() {
        return builtin.into();
    }
    match mime_guess::from_ext(&ext).first() {
        Some(m) if m.type_() == "text" => format!("{m}; charset=utf-8"),
        Some(m) => m.to_string(),
        // Go sniffs the content; the client's extension-less and unknown files (source maps) are text.
        None => "text/plain; charset=utf-8".into(),
    }
}

/// `gzhttp`'s default filter: everything but formats that are compressed already.
fn compressible(ct: &str) -> bool {
    !(ct.starts_with("image/") && ct != "image/svg+xml"
        || ct.starts_with("video/")
        || ct.starts_with("audio/")
        || ct.starts_with("font/woff")
        || ct.contains("zip")
        || ct.contains("compressed"))
}

/// A file's compressed bytes, with the modification time they were made from.
type Gz = (SystemTime, Arc<Vec<u8>>);

#[derive(Default)]
pub struct GzCache(Mutex<HashMap<PathBuf, Gz>>);

impl GzCache {
    fn get(&self, path: &PathBuf, mtime: SystemTime, data: &[u8]) -> Arc<Vec<u8>> {
        if let Some((t, gz)) = self.0.lock().unwrap().get(path)
            && *t == mtime
        {
            return gz.clone();
        }
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = enc.write_all(data);
        let gz = Arc::new(enc.finish().unwrap_or_default());
        self.0.lock().unwrap().insert(path.clone(), (mtime, gz.clone()));
        gz
    }
}

fn gz_cache() -> &'static GzCache {
    static C: std::sync::OnceLock<GzCache> = std::sync::OnceLock::new();
    C.get_or_init(GzCache::default)
}

/// `staticFilesHandler(http.StripPrefix(..., http.FileServer(http.Dir(dir))))` under `gzhttp.GzipHandler`.
async fn serve(dir: PathBuf, rel: &str, req_path: &str, headers: &HeaderMap, head: bool) -> Response {
    if req_path.ends_with('/') {
        return not_found();
    }
    let segs = clean(rel);
    let file = segs.iter().fold(dir, |p, s| p.join(s));
    let Ok(meta) = tokio::fs::metadata(&file).await else { return not_found() };
    let last = segs.last().copied().unwrap_or("");
    if meta.is_dir() {
        // `http.FileServer` sends a directory without its slash to the slashed path.
        let loc = format!("{last}/");
        return (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, loc)]).into_response();
    }
    if last == "index.html" {
        return (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, "./")]).into_response();
    }
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let last_modified = httpdate::fmt_http_date(mtime);
    let cache_control =
        if last == "remote_entry.js" { "no-cache, max-age=31556926, public" } else { "max-age=31556926, public" };
    let mut base = HeaderMap::new();
    base.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
    base.insert(header::LAST_MODIFIED, HeaderValue::from_str(&last_modified).unwrap());
    let unmodified = headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| httpdate::parse_http_date(v).ok())
        .is_some_and(|since| {
            let secs = |t: SystemTime| t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            secs(mtime) <= secs(since)
        });
    if unmodified {
        return (StatusCode::NOT_MODIFIED, base).into_response();
    }
    let Ok(data) = tokio::fs::read(&file).await else { return not_found() };
    let ct = content_type(last);
    base.insert(header::CONTENT_TYPE, HeaderValue::from_str(&ct).unwrap());
    base.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    let gzip_ok = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|e| e.trim().split(';').next() == Some("gzip")));
    // `gzhttp.DefaultMinSize`: smaller bodies go out as they are.
    let body: Vec<u8> = if gzip_ok && data.len() >= 1024 && compressible(&ct) {
        base.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        gz_cache().get(&file, mtime, &data).as_ref().clone()
    } else {
        base.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        data
    };
    base.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    if head {
        return (StatusCode::OK, base).into_response();
    }
    (StatusCode::OK, base, body).into_response()
}

async fn static_files(Path(rel): Path<String>, req: Request) -> Response {
    let head = req.method() == Method::HEAD;
    serve(client_dir(), &rel, req.uri().path(), req.headers(), head).await
}

async fn static_plugins(AxState(app): AxState<State>, Path(rel): Path<String>, req: Request) -> Response {
    let dir = match app.cfg.str("PluginSettings.ClientDirectory") {
        "" => "./client/plugins".to_string(),
        d => d.to_string(),
    };
    let head = req.method() == Method::HEAD;
    serve(PathBuf::from(dir), &rel, req.uri().path(), req.headers(), head).await
}

async fn robots(req: Request) -> Response {
    if req.uri().path().ends_with('/') {
        return not_found();
    }
    "User-agent: *\nDisallow: /\n".into_response()
}

/// `root`: the web client's page with the site's name and description, for every GET or HEAD outside `/api/`.
pub async fn root(app: &State) -> Response {
    let file = client_dir().join("root.html");
    let mut contents = match tokio::fs::read(&file).await {
        Ok(c) => c,
        Err(e) => {
            let mut r = (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}\n")).into_response();
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
            return r;
        }
    };
    // Go's `html.EscapeString`.
    let esc = |s: &str| {
        s.replace('&', "&amp;").replace('\'', "&#39;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&#34;")
    };
    let original = format!("<title>{}</title>", esc("Mattermost"));
    let site_name = match app.cfg.str("TeamSettings.SiteName") {
        "" => "Mattermost",
        s => s,
    };
    let description = app.cfg.str("TeamSettings.CustomDescriptionText");
    let mut modified = format!("<title>{}</title>", esc(site_name));
    if !description.is_empty() {
        modified.push_str(&format!("<meta property=\"og:description\" content=\"{}\" />", esc(description)));
    }
    if original != modified
        && let Ok(text) = String::from_utf8(contents.clone())
    {
        contents = text.replace(&original, &modified).into_bytes();
    }
    let mut r = contents.into_response();
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, max-age=31556926, public"));
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    r
}

pub fn routes() -> axum::Router<State> {
    use axum::routing::get;
    axum::Router::new()
        .route("/static/plugins/{*rel}", get(static_plugins))
        .route("/static/{*rel}", get(static_files))
        .route("/robots.txt", get(robots))
}

//! The web client as Go's web server hands it out: `/static/*` from the client directory, `/static/plugins/*`
//! from the plugins' client directory, `/robots.txt`, and `root.html` for any other GET outside `/api/`.
//!
//! The client directory is `EBACHAT_CLIENT_DIR` (Go finds `client/` next to its working directory or binary).
//! Static files are compressed as Go's `gzhttp` wrapper does in the default `WebserverMode` "gzip", with the
//! compressed bytes kept in memory per file and modification time.
//!
//! An error of a handler Go registers outside `/api/` (`/files/{id}/public`, `/login/sso/saml`) is its web error
//! page (`utils.RenderWebAppError`): an HTML page, or a redirect for a 3xx, to the client's signed
//! `/error?message=...&s=...`.

// Based on Mattermost server/channels/web/static.go, web.go, saml.go, handlers.go (handleContextError) and
// server/channels/utils/api.go, subpath.go, Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use axum::extract::{Path, Request, State as AxState};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::app::{App, AppError, State};

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

#[derive(Default)]
pub struct GzCache(Mutex<HashMap<PathBuf, (SystemTime, Arc<Vec<u8>>)>>);

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

/// `root`: the web client's page with the site's name and description, for every GET or HEAD outside `/api/`,
/// with the headers of Go's static handler (`NewStaticHandler`): no framing by other origins, and the script
/// policy root.html also states.
pub async fn root(app: &State) -> Response {
    let mut r = root_page(app).await;
    let h = r.headers_mut();
    h.insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
    if let Ok(v) = HeaderValue::from_str(&content_security_policy(app)) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    r
}

/// The `Content-Security-Policy` of Go's static handler: the frame ancestors, the hash of the inline script that
/// sets the subpath (`GetStaticScriptHashes`), and the `DeveloperFlags` this build is not a "dev" build for.
fn content_security_policy(app: &App) -> String {
    use base64::Engine;
    use sha2::Digest;
    let sub = subpath(app.cfg.str("ServiceSettings.SiteURL"));
    let sha = if sub == "/" {
        String::new()
    } else {
        let script = format!("window.publicPath='{sub}/static/'");
        format!(" 'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(script.as_bytes())))
    };
    let mut dev = String::new();
    for kv in app.cfg.str("ServiceSettings.DeveloperFlags").split(',').filter(|s| !s.is_empty()) {
        if let Some((k @ ("unsafe-eval" | "unsafe-inline"), "true")) = kv.split_once('=') {
            dev.push_str(&format!(" '{k}'"));
        }
    }
    format!("frame-ancestors 'self' {}; script-src 'self'{sha}{dev}", app.cfg.str("ServiceSettings.FrameAncestors"))
}

async fn root_page(app: &State) -> Response {
    let file = client_dir().join("root.html");
    let mut contents = match tokio::fs::read(&file).await {
        Ok(c) => c,
        Err(e) => {
            let mut r = (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}\n")).into_response();
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
            return r;
        }
    };
    let esc = html_escape;
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

/// Go's `html.EscapeString` (and `template.HTMLEscapeString`).
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('\'', "&#39;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&#34;")
}

// ---------- error pages ----------

/// `GetSubpathFromConfig`: the cleaned path of the site URL, `/` without one.
pub fn subpath(site_url: &str) -> String {
    let rest = site_url.split_once("://").map_or(site_url, |(_, r)| r);
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    let path = path.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() {
        return "/".into();
    }
    format!("/{}", clean(path).join("/"))
}

/// `url.Values.Encode`: sorted by key, keys and values query-escaped.
fn encode_query(params: &[(&str, &str)]) -> String {
    let mut p = params.to_vec();
    p.sort_by(|a, b| a.0.cmp(b.0));
    let esc = crate::opengraph::query_escape;
    p.iter().map(|(k, v)| format!("{}={}", esc(k), esc(v))).collect::<Vec<_>>().join("&")
}

/// `template.JSEscapeString`.
fn js_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '<' => out.push_str("\\u003C"),
            '>' => out.push_str("\\u003E"),
            '&' => out.push_str("\\u0026"),
            '=' => out.push_str("\\u003D"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `RenderWebError`: to `/error?<params>&s=<signature>`, the path and query signed with the server's
/// `AsymmetricSigningKey` (ECDSA P-256 over SHA-256, ASN.1, URL-safe base64). A redirect for a 3xx status,
/// otherwise an HTML page that goes there.
pub async fn render_web_error(app: &App, status: StatusCode, params: &[(&str, &str)]) -> Response {
    use base64::Engine;
    use p256::ecdsa::signature::Signer;
    let sub = subpath(app.cfg.str("ServiceSettings.SiteURL"));
    let error_path = if sub == "/" { "/error".to_string() } else { format!("{sub}/error") };
    let base = format!("{error_path}?{}", encode_query(params));
    let Ok(keys) = app.integration_keys().await else {
        // `http.Error(w, "", http.StatusInternalServerError)`.
        let mut r = (StatusCode::INTERNAL_SERVER_ERROR, "\n").into_response();
        r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
        return r;
    };
    let sig: p256::ecdsa::Signature = keys.signing.sign(base.as_bytes());
    let destination = format!("{base}&s={}", base64::engine::general_purpose::URL_SAFE.encode(sig.to_der().as_bytes()));
    if status.is_redirection() {
        // `http.Redirect` after the handler set its JSON content type: no body.
        let mut r = status.into_response();
        let h = r.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Ok(v) = HeaderValue::from_str(&destination) {
            h.insert(header::LOCATION, v);
        }
        return r;
    }
    let href = html_escape(&destination);
    let body = format!(
        "<!DOCTYPE html><html><head></head>\n\
         <body onload=\"window.location = '{}'\">\n\
         <noscript><meta http-equiv=\"refresh\" content=\"0; url={href}\"></noscript>\n\
         <!-- web error message -->\n\
         <a href=\"{href}\" style=\"color: #c0c0c0;\">...</a>\n\
         </body></html>\n",
        html_escape(&js_escape(&destination)),
    );
    let mut r = (status, body).into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    r
}

/// `handleContextError` for a handler outside `/api/`, `/hooks/` and the OAuth API paths: the JSON error for the
/// mobile app (`X-Mobile-App`), otherwise the web error page with the error's message (`RenderWebAppError`).
pub async fn web_error(app: &App, headers: &HeaderMap, mut e: AppError) -> Response {
    if app.cfg.bool("ServiceSettings.EnableHardenedMode") && e.status_code >= 500 {
        e.id.clear();
        e.message = "Internal Server Error".into();
        e.detailed_error.clear();
        e.status_code = 500;
    }
    if headers.get("x-mobile-app").is_some_and(|v| !v.is_empty()) {
        return e.into_response();
    }
    let status = StatusCode::from_u16(e.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut r = render_web_error(app, status, &[("message", &e.message)]).await;
    if let Ok(v) = e.request_id.parse() {
        r.headers_mut().insert("x-request-id", v);
    }
    r
}

/// `loginWithSaml` (GET) and `completeSaml` (POST): SAML is an Enterprise feature, never available here.
async fn saml(AxState(app): AxState<State>, headers: HeaderMap) -> Response {
    let e = AppError::new(
        StatusCode::FOUND,
        "api.user.saml.not_available.app_error",
        "SAML 2.0 is not configured or supported on this server.",
    );
    web_error(&app, &headers, e).await
}

pub fn routes() -> axum::Router<State> {
    use axum::routing::get;
    axum::Router::new()
        .route("/login/sso/saml", get(saml).post(saml))
        .route("/static/plugins/{*rel}", get(static_plugins))
        .route("/static/{*rel}", get(static_files))
        .route("/robots.txt", get(robots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_page_escaping() {
        assert_eq!(subpath("http://localhost:8065"), "/");
        assert_eq!(subpath("https://x.org/chat/"), "/chat");
        assert_eq!(subpath("https://x.org/a//b/../c?q=1"), "/a/c");
        assert_eq!(encode_query(&[("type", "a b"), ("message", "x&y=z.")]), "message=x%26y%3Dz.&type=a+b");
        let d = "/error?message=Public+links+have+been+disabled.&s=MEQ=";
        // `=` for `=`, `&` for `&`.
        let (eq, amp) = (format!("{}u003D", char::from(92)), format!("{}u0026", char::from(92)));
        let want = format!("/error?message{eq}Public+links+have+been+disabled.{amp}s{eq}MEQ{eq}");
        assert_eq!(html_escape(&js_escape(d)), want);
        assert_eq!(html_escape(d), "/error?message=Public+links+have+been+disabled.&amp;s=MEQ=");
    }
}

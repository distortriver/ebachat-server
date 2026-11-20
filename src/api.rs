//! REST API v4: the routes of every area, and what is answered as a server without plugins or a license.

use axum::extract::Path;
use axum::http::Uri;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

use crate::app::{AppError, State};

/// Go's answer for a path (or a method) the API does not have.
async fn not_found(uri: Uri) -> impl IntoResponse {
    route_not_found(uri.path())
}

/// Go's router 404, also for a path whose parameter fails the route's pattern (`{timestamp:[0-9]+}`).
pub fn route_not_found(path: &str) -> AppError {
    let mut e = AppError::not_found("api.context.404.app_error", "Sorry, we could not find the page.").detail(format!(
        "There doesn't appear to be an api call for the url='{path}'.  Typo? are you missing a team_id or user_id as part of the url?"
    ));
    e.request_id.clear();
    e
}

/// `LOG_REQUESTS=1`: one line per request on stderr (method, path, status, ms), to watch a real client.
async fn log_request(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let (method, uri, t) = (req.method().clone(), req.uri().clone(), std::time::Instant::now());
    let r = next.run(req).await;
    eprintln!("{} {} {} {}ms", method, uri, r.status().as_u16(), t.elapsed().as_millis());
    r
}

/// The headers `web.Handler` puts on every API answer.
async fn go_headers(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::http::HeaderValue;
    let get = req.method() == axum::http::Method::GET;
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    if !h.contains_key("x-request-id") {
        h.insert("x-request-id", HeaderValue::from_str(&crate::app::new_id()).unwrap());
    }
    h.insert("x-version-id", HeaderValue::from_static(concat!("12.1.0.12.1.0.", "0", ".false")));
    h.insert("permissions-policy", HeaderValue::from_static(""));
    h.entry("x-content-type-options").or_insert(HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if get && !h.contains_key("expires") && !h.contains_key("cache-control") {
        h.insert("expires", HeaderValue::from_static("0"));
    }
    r
}

pub fn router(app: State) -> Router {
    crate::app::DEVELOPER.store(app.cfg.bool("ServiceSettings.EnableDeveloper"), std::sync::atomic::Ordering::Relaxed);
    crate::outgoing::spawn_worker(app.clone());
    let r = routes(app).layer(axum::middleware::from_fn(go_headers));
    if std::env::var("LOG_REQUESTS").is_ok_and(|v| v == "1") { r.layer(axum::middleware::from_fn(log_request)) } else { r }
}

/// Every cache's size and hit, miss and invalidation counters (Go exports them as Prometheus metrics); system
/// admins only.
async fn cache_stats(
    axum::extract::State(app): axum::extract::State<State>,
    crate::app::Auth(s): crate::app::Auth,
) -> crate::app::ApiResult<axum::Json<std::collections::BTreeMap<&'static str, crate::cache::Stats>>> {
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    Ok(axum::Json(app.caches.stats()))
}

/// `/static/emoji/{file}`: the system emoji images of the web client, which EBAChat shows from the server too.
/// Served as Go serves `/static/` from its client directory (`client/emoji/...`); `EBACHAT_CLIENT_DIR` points
/// elsewhere. No other web client files are served.
async fn static_emoji(Path(file): Path<String>) -> axum::response::Response {
    use axum::http::{HeaderValue, StatusCode, header};
    let ok = file.len() < 64
        && file.strip_suffix(".png").is_some_and(|stem| {
            !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        });
    let dir = std::env::var("EBACHAT_CLIENT_DIR").unwrap_or_else(|_| "client".into());
    let Some(data) = ok.then(|| std::fs::read(format!("{dir}/emoji/{file}")).ok()).flatten() else {
        return (StatusCode::NOT_FOUND, "404 page not found\n").into_response();
    };
    let mut r = data.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=31556926, public"));
    r
}

fn routes(app: State) -> Router {
    Router::new()
        .route("/static/emoji/{file}", get(static_emoji))
        .route("/api/v4/ebachat/caches", get(cache_stats))
        .route("/api/v4/websocket", get(crate::hub::connect))
        .merge(crate::users::routes())
        .merge(crate::user_lists::routes())
        .merge(crate::user_edit::routes())
        .merge(crate::user_image::routes())
        .merge(crate::system::routes())
        .merge(crate::config_api::routes())
        .merge(crate::admin::routes())
        .merge(crate::unlicensed::routes())
        .merge(crate::commands::routes())
        .merge(crate::command_admin::routes())
        .merge(crate::outgoing::routes())
        .merge(crate::status::routes())
        .merge(crate::channels::routes())
        .merge(crate::sidebar::routes())
        .merge(crate::posts::routes())
        .merge(crate::file_api::routes())
        .merge(crate::emoji::routes())
        .merge(crate::preferences::routes())
        .merge(crate::webhooks::routes())
        .merge(crate::integrations::routes())
        .merge(crate::teams::routes())
        .merge(crate::team_admin::routes())
        .merge(crate::schemes::routes())
        .merge(crate::roles::routes())
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(app)
}

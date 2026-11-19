//! REST API v4: the routes of every area, and what is answered as a server without plugins or a license.

use axum::http::Uri;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

use crate::app::{AppError, State};

/// Go's answer for a path (or a method) the API does not have.
async fn not_found(uri: Uri) -> impl IntoResponse {
    let mut e = AppError::not_found("api.context.404.app_error", "Sorry, we could not find the page.").detail(format!(
        "There doesn't appear to be an api call for the url='{}'.  Typo? are you missing a team_id or user_id as part of the url?",
        uri.path()
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
    let r = routes(app).layer(axum::middleware::from_fn(go_headers));
    if std::env::var("LOG_REQUESTS").is_ok_and(|v| v == "1") { r.layer(axum::middleware::from_fn(log_request)) } else { r }
}

fn routes(app: State) -> Router {
    Router::new()
        .route("/api/v4/websocket", get(crate::hub::connect))
        .merge(crate::users::routes())
        .merge(crate::user_lists::routes())
        .merge(crate::user_edit::routes())
        .merge(crate::user_image::routes())
        .merge(crate::system::routes())
        .merge(crate::commands::routes())
        .merge(crate::status::routes())
        .merge(crate::channels::routes())
        .merge(crate::sidebar::routes())
        .merge(crate::posts::routes())
        .merge(crate::file_api::routes())
        .merge(crate::emoji::routes())
        .merge(crate::preferences::routes())
        .merge(crate::teams::routes())
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(app)
}

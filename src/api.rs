//! REST API v4: the routes of every area, and what is answered as a server without plugins or a license.

use axum::http::Uri;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

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

pub fn router(app: State) -> Router {
    let r = routes(app);
    if std::env::var("LOG_REQUESTS").is_ok_and(|v| v == "1") { r.layer(axum::middleware::from_fn(log_request)) } else { r }
}

fn routes(app: State) -> Router {
    Router::new()
        .route("/api/v4/system/ping", get(|| async { Json(json!({"status": "OK"})) }))
        .route("/api/v4/websocket", get(crate::hub::connect))
        .merge(crate::users::routes())
        .merge(crate::status::routes())
        .merge(crate::channels::routes())
        .merge(crate::sidebar::routes())
        .merge(crate::posts::routes())
        .merge(crate::file_api::routes())
        .merge(crate::emoji::routes())
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(app)
}

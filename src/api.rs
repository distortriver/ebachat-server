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

pub fn router(app: State) -> Router {
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

//! REST API v4: the routes of every area, and what is answered as a server without plugins or a license.

use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::app::State;

pub fn router(app: State) -> Router {
    Router::new()
        .route("/api/v4/system/ping", get(|| async { Json(json!({"status": "OK"})) }))
        .route("/api/v4/websocket", get(crate::hub::connect))
        .merge(crate::users::routes())
        .merge(crate::user_lists::routes())
        .merge(crate::status::routes())
        .merge(crate::channels::routes())
        .merge(crate::sidebar::routes())
        .merge(crate::posts::routes())
        .merge(crate::file_api::routes())
        .merge(crate::emoji::routes())
        .merge(crate::preferences::routes())
        .merge(crate::teams::routes())
        .with_state(app)
}

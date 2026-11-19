//! Custom emoji.
//!
//! STUB: owned by the users branch. Contract: [`App::emojis_by_names`].

use std::sync::Arc;

use axum::Router;

use crate::app::{ApiResult, App, State};
use crate::model::Emoji;

pub fn routes() -> Router<State> {
    Router::new()
}

impl App {
    /// `EmojiStore.GetMultipleByName` through the emoji caches: the custom emoji of these names that exist.
    pub async fn emojis_by_names(&self, _names: &[String]) -> ApiResult<Vec<Arc<Emoji>>> {
        Ok(vec![])
    }
}

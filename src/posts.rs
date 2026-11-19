//! Posts: creation (counters, threads, mentions, the `posted` event), reading with the post caches, editing,
//! deleting, pins, reactions, search, and what `PreparePostForClient` adds.
//!
//! STUB: owned by the posts branch. The contract other modules use is [`App::create_post`].

use axum::Router;

use crate::app::{ApiResult, App, State};
use crate::model::Post;

pub fn routes() -> Router<State> {
    Router::new()
}

impl App {
    /// `App.CreatePost`: saves the post (ids, times, channel counters, thread), notifies (mentions, unread counts,
    /// thread memberships) and publishes `posted`. Other modules post system messages through it
    /// (`join_channel`, `leave_channel`, header changes, ...). `post.id` empty: a new id.
    pub async fn create_post(&self, post: Post, _set_online: bool) -> ApiResult<Post> {
        let _ = &post;
        Err(crate::app::AppError::not_implemented("create_post"))
    }
}

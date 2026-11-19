//! Files: uploads (thumbnails, previews, mini previews), downloads, links, search, and the file infos of posts.
//!
//! STUB: owned by the files branch. Contracts: [`App::file_infos_for_post`], [`App::attach_files_to_post`].

use std::sync::Arc;

use axum::Router;

use crate::app::{ApiResult, App, State};
use crate::model::FileInfo;

pub fn routes() -> Router<State> {
    Router::new()
}

impl App {
    /// `FileInfoStore.GetForPost` through the file info cache.
    pub async fn file_infos_for_post(&self, _post_id: &str, _include_deleted: bool) -> ApiResult<Arc<Vec<FileInfo>>> {
        Ok(Arc::new(vec![]))
    }

    /// `attachFilesToPost`: the uploader's unattached files of the channel get the post id; returns the ids
    /// attached.
    pub async fn attach_files_to_post(
        &self,
        _post_id: &str,
        _channel_id: &str,
        _user_id: &str,
        file_ids: &[String],
    ) -> ApiResult<Vec<String>> {
        Ok(file_ids.to_vec())
    }
}

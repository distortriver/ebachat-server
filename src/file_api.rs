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

/// What a link in a message gives the post's metadata: an OpenGraph preview, or an image's size.
#[derive(Clone, Debug, Default)]
pub struct LinkMeta {
    /// `{"type", "url", "title", "description", "site_name", "images": [...]}` as Go's `opengraph.OpenGraph`.
    pub opengraph: Option<serde_json::Value>,
    /// `PostImage`: `{"width", "height", "format", "frame_count"}`.
    pub image: Option<serde_json::Value>,
}

impl App {
    /// `getLinkMetadata` through the open graph cache: what `url` is, fetched once (`EnableLinkPreviews`,
    /// `RestrictLinkPreviews` apply). None when it gives nothing.
    pub async fn link_metadata(&self, _url: &str) -> Option<Arc<LinkMeta>> {
        None
    }

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

//! Channels and channel members: reading through the channel caches, creating (open, private, direct, group),
//! joining and leaving, viewing (unread counts), notify props, stats.
//!
//! STUB: owned by the channels branch. Contracts other modules use: [`App::mark_viewed`],
//! [`App::channel_member`].

use std::collections::BTreeMap;

use axum::Router;

use crate::app::{ApiResult, App, State};
use crate::model::{Channel, ChannelMember};

pub fn routes() -> Router<State> {
    Router::new()
}

impl App {
    /// `MarkChannelsAsViewed` for `user_id`: `last_viewed_at` per channel, `multiple_channels_viewed` sent.
    pub async fn mark_viewed(&self, _ids: &[String], _user_id: &str) -> ApiResult<BTreeMap<String, i64>> {
        Ok(BTreeMap::new())
    }

    /// `AddChannelMember` / `addUserToChannel`: the membership (default notify props, scheme roles), sidebar,
    /// caches, `user_added`, and the "joined"/"added" system message (`actor_id` == `user_id`: joined). Teams use
    /// it for the default channels of a team.
    pub async fn add_channel_member(
        &self,
        _channel: &Channel,
        _user_id: &str,
        _actor_id: &str,
        _post_root_id: &str,
    ) -> ApiResult<ChannelMember> {
        Err(crate::app::AppError::not_implemented("add_channel_member"))
    }

    /// `GetChannelMember` with the scheme-derived roles.
    pub async fn channel_member(&self, channel_id: &str, user_id: &str) -> ApiResult<Option<ChannelMember>> {
        Ok(crate::perm::channel_members(self, "cm.channelid = $1 AND cm.userid = $2", &[channel_id, user_id])
            .await?
            .into_iter()
            .next())
    }
}

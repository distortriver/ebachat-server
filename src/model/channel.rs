// Based on Mattermost server/public/model/channel.go and channel_member.go, Copyright (c) 2015-present
// Mattermost, Inc. Licensed under AGPL-3.0.

use ebachat_macros::model;
use serde_json::Value;
use sqlx::types::Json;

use super::{StringInterface, StringMap};

pub const OPEN: &str = "O";
pub const PRIVATE: &str = "P";
pub const DIRECT: &str = "D";
pub const GROUP: &str = "G";
/// The backing channel of a space (`ChannelTypeSpace`): opaque to the `/channels` API.
pub const SPACE: &str = "S";
/// Board channels (`ChannelTypeOpenBoard`, `ChannelTypePrivateBoard`): served by `/boards`, not `/channels`.
pub const OPEN_BOARD: &str = "BO";
pub const PRIVATE_BOARD: &str = "BP";

/// The store's `messageChannelTypes`: the types its generic lookups, lists and searches return.
pub fn is_message_type(t: &str) -> bool {
    matches!(t, OPEN | PRIVATE | DIRECT | GROUP)
}

#[model(table = "channels")]
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub id: String,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    pub team_id: String,
    #[mm(pgtype = "channel_type")]
    pub r#type: String,
    pub display_name: String,
    pub name: String,
    pub header: String,
    pub purpose: String,
    pub last_post_at: i64,
    pub total_msg_count: i64,
    pub extra_update_at: i64,
    pub creator_id: String,
    pub scheme_id: Option<String>,
    #[mm(transient)]
    pub props: Option<StringInterface>,
    pub group_constrained: Option<bool>,
    #[mm(json = "autotranslation", column = "autotranslation")]
    pub auto_translation: bool,
    pub shared: Option<bool>,
    pub total_msg_count_root: i64,
    #[mm(transient)]
    pub policy_id: Option<String>,
    pub last_root_post_at: i64,
    pub banner_info: Option<Json<Value>>,
    #[mm(transient)]
    pub policy_enforced: bool,
    #[mm(transient)]
    pub policy_auto_add: bool,
    #[mm(transient)]
    pub policy_is_active: bool,
    pub default_category_name: String,
    #[mm(transient)]
    pub managed_category_name: String,
    pub discoverable: bool,
    pub disable_join_leave_messages: bool,
}

impl Channel {
    pub fn is_group_or_direct(&self) -> bool {
        self.r#type == DIRECT || self.r#type == GROUP
    }
    /// `IsBoard`.
    pub fn is_board(&self) -> bool {
        self.r#type == OPEN_BOARD || self.r#type == PRIVATE_BOARD
    }
    /// `IsSpace`.
    pub fn is_space(&self) -> bool {
        self.r#type == SPACE
    }
    /// The other user of a direct channel named `a__b`.
    pub fn other_user(&self, me: &str) -> Option<String> {
        let (a, b) = self.name.split_once("__")?;
        Some(if a == me { b } else { a }.to_string())
    }
}

/// `GetDMNameFromIds`: the two ids sorted, joined by `__`.
pub fn direct_name(a: &str, b: &str) -> String {
    if a > b { format!("{b}__{a}") } else { format!("{a}__{b}") }
}

#[model(table = "channelmembers")]
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMember {
    pub channel_id: String,
    pub user_id: String,
    pub roles: String,
    pub msg_count: i64,
    pub mention_count: i64,
    pub mention_count_root: i64,
    pub urgent_mention_count: i64,
    pub msg_count_root: i64,
    pub notify_props: StringMap,
    pub scheme_guest: bool,
    pub scheme_user: bool,
    pub scheme_admin: bool,
    #[mm(transient)]
    pub explicit_roles: String,
    #[mm(json = "autotranslation_disabled", column = "autotranslationdisabled")]
    pub auto_translation_disabled: bool,
    // Go's `ChannelMember.MarshalJSON` puts these two last, and leaves them out when
    // `SanitizeForCurrentUser` set them to -1 (another user's membership).
    #[serde(skip_serializing_if = "is_sanitized")]
    pub last_viewed_at: i64,
    #[serde(skip_serializing_if = "is_sanitized")]
    pub last_update_at: i64,
}

fn is_sanitized(v: &i64) -> bool {
    *v == -1
}

/// `GetDefaultChannelNotifyProps`.
pub fn default_member_notify_props() -> StringMap {
    let mut m = std::collections::BTreeMap::new();
    for (k, v) in [
        ("desktop", "default"),
        ("mark_unread", "all"),
        ("push", "default"),
        ("email", "default"),
        ("ignore_channel_mentions", "default"),
        ("channel_auto_follow_threads", "off"),
    ] {
        m.insert(k.to_string(), v.to_string());
    }
    Json(m)
}

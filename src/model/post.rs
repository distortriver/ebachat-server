// Based on Mattermost server/public/model/post.go, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use ebachat_macros::model;
use serde_json::Value;

use super::{StringArray, StringInterface, User};

#[model(table = "posts")]
#[derive(Debug, Clone, PartialEq)]
pub struct Post {
    pub id: String,
    pub create_at: i64,
    pub update_at: i64,
    pub edit_at: i64,
    pub delete_at: i64,
    pub is_pinned: bool,
    pub user_id: String,
    pub channel_id: String,
    pub root_id: String,
    pub original_id: String,
    pub message: String,
    #[mm(transient, omitempty)]
    pub message_source: String,
    pub r#type: String,
    pub props: StringInterface,
    pub hashtags: String,
    #[mm(skip_json)]
    pub filenames: StringArray,
    pub file_ids: StringArray,
    #[mm(transient)]
    pub pending_post_id: String,
    #[mm(omitempty)]
    pub has_reactions: bool,
    #[mm(omitempty)]
    pub remote_id: Option<String>,
    #[mm(transient)]
    pub reply_count: i64,
    #[mm(transient)]
    pub last_reply_at: i64,
    #[mm(transient)]
    pub participants: Option<Vec<User>>,
    #[mm(transient, omitempty)]
    pub is_following: Option<bool>,
    #[mm(transient, omitempty)]
    pub metadata: Option<Value>,
}

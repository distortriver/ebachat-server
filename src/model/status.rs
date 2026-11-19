// Based on Mattermost server/public/model/status.go, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use ebachat_macros::model;

pub const ONLINE: &str = "online";
pub const OFFLINE: &str = "offline";
pub const AWAY: &str = "away";
pub const DND: &str = "dnd";
pub const OUT_OF_OFFICE: &str = "ooo";
/// `StatusMinUpdateTime`: an unchanged online status is written at most this often.
pub const MIN_UPDATE_TIME: i64 = 120_000;

#[model(table = "status")]
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    #[mm(column = "userid")]
    pub user_id: String,
    pub status: String,
    pub manual: bool,
    pub last_activity_at: i64,
    /// Server-side only: `ToJSON`/`StatusListToJSON` drop it.
    #[mm(transient, skip_json)]
    pub active_channel: String,
    pub dnd_end_time: i64,
    #[mm(skip_json)]
    pub prev_status: String,
}

impl Status {
    /// A user with no `Status` row: offline, as `GetUserStatusesByIds` fills them in.
    pub fn offline(user_id: &str) -> Status {
        Status { user_id: user_id.into(), status: OFFLINE.into(), ..Default::default() }
    }
}

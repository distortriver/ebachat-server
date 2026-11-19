// Based on Mattermost server/public/model/session.go, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use ebachat_macros::model;

use super::StringMap;

#[model(table = "sessions")]
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub token: String,
    pub create_at: i64,
    pub expires_at: i64,
    pub last_activity_at: i64,
    pub user_id: String,
    pub device_id: String,
    #[mm(column = "voipdeviceid")]
    pub voip_device_id: String,
    pub roles: String,
    #[mm(column = "isoauth")]
    pub is_oauth: bool,
    pub expired_notify: bool,
    pub props: StringMap,
}

impl Session {
    pub fn is_expired(&self, now: i64) -> bool {
        self.expires_at > 0 && now > self.expires_at
    }
    pub fn is_system_admin(&self) -> bool {
        self.roles.split(' ').any(|r| r == "system_admin")
    }
}

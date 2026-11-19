// Based on Mattermost server/public/model/session.go, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use ebachat_macros::model;

use super::{StringMap, TeamMember};

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
    #[mm(transient)]
    pub team_members: Vec<TeamMember>,
    #[mm(transient)]
    pub local: bool,
}

pub const PROP_TYPE: &str = "type";
pub const TYPE_USER_ACCESS_TOKEN: &str = "UserAccessToken";
pub const PROP_IS_BOT: &str = "is_bot";
pub const PROP_CSRF: &str = "csrf";

impl Session {
    pub fn is_expired(&self, now: i64) -> bool {
        self.expires_at > 0 && now > self.expires_at
    }
    pub fn is_system_admin(&self) -> bool {
        self.roles.split(' ').any(|r| r == "system_admin")
    }
    pub fn prop(&self, k: &str) -> &str {
        self.props.0.get(k).map(String::as_str).unwrap_or("")
    }
    pub fn is_user_access_token(&self) -> bool {
        self.prop(PROP_TYPE) == TYPE_USER_ACCESS_TOKEN
    }
    pub fn is_mobile_app(&self) -> bool {
        !self.device_id.is_empty() || !self.prop("mobile_version").is_empty()
    }
    pub fn is_sso_login(&self) -> bool {
        self.is_oauth || !self.prop("sso").is_empty()
    }
    /// `Session.Sanitize`: the token never leaves the server again.
    pub fn sanitize(&mut self) {
        self.token.clear();
    }
}

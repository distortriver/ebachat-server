// Based on Mattermost server/public/model/user.go, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use ebachat_macros::model;
use serde_json::Value;
use sqlx::types::Json;

use super::StringMap;

#[model(table = "users")]
#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: String,
    #[mm(omitempty)]
    pub create_at: i64,
    #[mm(omitempty)]
    pub update_at: i64,
    pub delete_at: i64,
    pub username: String,
    #[mm(omitempty)]
    pub password: String,
    #[mm(omitempty)]
    pub auth_data: Option<String>,
    pub auth_service: String,
    pub email: String,
    #[mm(omitempty)]
    pub email_verified: bool,
    pub nickname: String,
    pub first_name: String,
    pub last_name: String,
    pub position: String,
    pub roles: String,
    #[mm(omitempty)]
    pub allow_marketing: bool,
    #[mm(omitempty)]
    pub props: StringMap,
    #[mm(omitempty)]
    pub notify_props: StringMap,
    #[mm(omitempty)]
    pub last_password_update: i64,
    #[mm(omitempty)]
    pub last_picture_update: i64,
    #[mm(omitempty)]
    pub failed_attempts: i32,
    pub locale: String,
    pub timezone: StringMap,
    #[mm(omitempty)]
    pub mfa_active: bool,
    #[mm(omitempty)]
    pub mfa_secret: String,
    #[mm(omitempty)]
    pub remote_id: Option<String>,
    #[mm(transient, omitempty)]
    pub last_activity_at: i64,
    #[mm(transient, omitempty)]
    pub is_bot: bool,
    #[mm(transient, omitempty)]
    pub bot_description: String,
    #[mm(transient, omitempty)]
    pub bot_last_icon_update: i64,
    #[mm(transient, omitempty)]
    pub terms_of_service_id: String,
    #[mm(transient, omitempty)]
    pub terms_of_service_create_at: i64,
    #[mm(transient)]
    pub disable_welcome_email: bool,
    #[mm(omitempty)]
    pub last_login: i64,
    #[mm(omitempty)]
    /// JSON `null` in rows Go wrote for bots.
    pub mfa_used_timestamps: Json<Option<Vec<Value>>>,
}

impl User {
    /// `User.Sanitize(map[string]bool{})`: what never leaves the server, for the user's own record.
    pub fn sanitize_own(&mut self) {
        self.password.clear();
        self.mfa_secret.clear();
        self.mfa_used_timestamps = Default::default();
        self.last_login = 0;
    }

    /// `User.Sanitize(options)` with non-empty options.
    pub fn sanitize(&mut self, o: Show) {
        self.sanitize_own();
        if !o.email {
            self.email.clear();
            self.props.0.remove("RemoteEmail");
        }
        if !o.fullname {
            self.first_name.clear();
            self.last_name.clear();
        }
        self.last_password_update = 0;
        if !o.admin {
            self.auth_service.clear();
            self.auth_data = Some(String::new());
        }
    }

    /// `App.SanitizeProfile`: `ClearNonProfileFields` and `Sanitize` with the privacy settings.
    pub fn sanitize_profile(&mut self, o: Show) {
        self.email_verified = false;
        self.allow_marketing = false;
        if !o.admin {
            self.auth_data = Some(String::new());
            self.notify_props = Default::default();
            self.failed_attempts = 0;
        }
        self.sanitize(o);
    }

    pub fn is_guest(&self) -> bool {
        self.roles.split_whitespace().any(|r| r == "system_guest")
    }
    pub fn is_system_admin(&self) -> bool {
        self.roles.split_whitespace().any(|r| r == "system_admin")
    }
    /// `GetFullName`.
    pub fn full_name(&self) -> String {
        match (self.first_name.is_empty(), self.last_name.is_empty()) {
            (false, false) => format!("{} {}", self.first_name, self.last_name),
            (false, true) => self.first_name.clone(),
            (true, false) => self.last_name.clone(),
            (true, true) => String::new(),
        }
    }
    /// `GetDisplayName(TeammateNameDisplay)`.
    pub fn display_name(&self, setting: &str) -> String {
        let name = self.username.clone();
        match setting {
            "nickname_full_name" if !self.nickname.is_empty() => self.nickname.clone(),
            "nickname_full_name" | "full_name" if !self.full_name().is_empty() => self.full_name(),
            _ => name,
        }
    }
    pub fn notify_prop(&self, k: &str) -> &str {
        self.notify_props.0.get(k).map(String::as_str).unwrap_or("")
    }
}

/// What a viewer may see of other users' profiles (`GetSanitizeOptions`).
#[derive(Clone, Copy, Debug)]
pub struct Show {
    pub email: bool,
    pub fullname: bool,
    pub admin: bool,
}

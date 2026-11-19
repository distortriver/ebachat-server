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
    pub mfa_used_timestamps: Json<Vec<Value>>,
}

impl User {
    /// `User.Sanitize`: what never leaves the server, and with `options`, what the viewer may not see.
    pub fn sanitize(&mut self, email: bool, fullname: bool, passwordupdate: bool, authservice: bool, authdata: bool) {
        self.password.clear();
        self.mfa_secret.clear();
        self.mfa_used_timestamps = Default::default();
        self.last_login = 0;
        if !email {
            self.email.clear();
            self.props.0.remove("RemoteEmail");
        }
        if !fullname {
            self.first_name.clear();
            self.last_name.clear();
        }
        if !passwordupdate {
            self.last_password_update = 0;
        }
        if !authservice {
            self.auth_service.clear();
        }
        if !authdata {
            self.auth_data = Some(String::new());
        }
    }
}

impl User {
    /// `App.SanitizeProfile` with the default privacy settings (`ShowEmailAddress`, `ShowFullName` on).
    pub fn sanitize_profile(&mut self, as_admin: bool) {
        // ClearNonProfileFields
        self.password.clear();
        self.mfa_secret.clear();
        self.mfa_used_timestamps = Default::default();
        self.email_verified = false;
        self.allow_marketing = false;
        self.last_password_update = 0;
        if !as_admin {
            self.auth_data = Some(String::new());
            self.notify_props = Default::default();
            self.failed_attempts = 0;
        }
        self.sanitize(true, true, false, as_admin, as_admin);
    }
}

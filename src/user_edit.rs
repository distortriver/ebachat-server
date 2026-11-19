//! Changing users: profile patches, passwords, custom statuses, sign-up, the emails the API promises, and
//! sessions.

// Based on Mattermost server/channels/api4/user.go, api4/status.go, app/user.go, app/status.go (custom status),
// app/users and model/user.go (PreUpdate, IsValid), Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State as St};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{ApiResult, App, AppError, Auth, Body, MaybeAuth, State, is_valid_id, now_ms};
use crate::db::Table;
use crate::hub::Broadcast;
use crate::model::{Preference, Session, Show, User};
use crate::password;
use crate::perm;
use crate::preferences::user_param;
use crate::users::{missing_account, ok};

pub fn routes() -> Router<State> {
    Router::new()
        .route("/api/v4/users", post(create_user))
        .route("/api/v4/users/{user_id}/patch", put(patch_user))
        .route("/api/v4/users/{user_id}/password", put(update_password))
        .route("/api/v4/users/{user_id}/status/custom", put(set_custom_status).delete(remove_custom_status))
        .route("/api/v4/users/{user_id}/status/custom/recent/delete", post(remove_recent_custom_status))
        .route("/api/v4/users/password/reset/send", post(send_password_reset))
        .route("/api/v4/users/email/verify/send", post(send_verification))
        .route("/api/v4/users/{user_id}/sessions", get(get_sessions))
        .route("/api/v4/users/sessions/device", put(device_props))
}

const LOCALES: &[&str] = &[
    "bg", "cs", "da", "de", "en", "en-AU", "es", "fa", "fi", "fr", "hr", "hu", "it", "ja", "ko", "lt", "nb-NO", "nl",
    "pl", "pt", "pt-BR", "ro", "ru", "sv", "tr", "uk", "vi", "zh-CN", "zh-TW",
];

fn invalid_user(field: &str) -> AppError {
    AppError::bad_request(&format!("model.user.is_valid.{field}.app_error"), "Invalid user.")
}

/// `IsValidUsername`.
pub fn valid_username(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-' || b == b'_')
        && !matches!(s, "all" | "channel" | "matterbot" | "system")
}

/// `IsValidEmail`: lower case, one plain address.
fn valid_email(s: &str) -> bool {
    if s.chars().any(|c| c.is_uppercase() || c.is_whitespace() || c == '<' || c == '>') {
        return false;
    }
    let Some((local, domain)) = s.rsplit_once('@') else { return false };
    !local.is_empty() && !domain.is_empty() && !domain.starts_with('.') && !domain.ends_with('.')
}

/// `User.IsValid` for the fields this server writes.
fn check_user(u: &User) -> ApiResult<()> {
    if !is_valid_id(&u.id) {
        return Err(invalid_user("id"));
    }
    if !valid_username(&u.username) {
        return Err(invalid_user("username"));
    }
    if u.email.len() > 128 || u.email.is_empty() || !valid_email(&u.email) {
        return Err(invalid_user("email"));
    }
    let runes = |s: &str| s.chars().count();
    if runes(&u.nickname) > 64 {
        return Err(invalid_user("nickname"));
    }
    if runes(&u.position) > 128 {
        return Err(invalid_user("position"));
    }
    if runes(&u.first_name) > 64 {
        return Err(invalid_user("first_name"));
    }
    if runes(&u.last_name) > 64 {
        return Err(invalid_user("last_name"));
    }
    if u.locale.len() > 5 || !LOCALES.contains(&u.locale.as_str()) && !u.locale.is_empty() {
        return Err(invalid_user("locale"));
    }
    if serde_json::to_string(&u.timezone).map(|s| s.chars().count()).unwrap_or(0) > 256 {
        return Err(invalid_user("timezone_limit"));
    }
    if let Some(cs) = u.props.0.get("customStatus")
        && !cs.is_empty()
        && serde_json::from_str::<CustomStatus>(cs).is_err()
    {
        return Err(AppError::bad_request("model.user.is_valid.invalidProperty.app_error", "Invalid properties."));
    }
    Ok(())
}

/// `SanitizeUnicode`: no bidi controls or zero-width characters.
fn sanitize_unicode(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(*c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0xFEFF))
        .collect()
}

/// `SetDefaultNotifications`.
pub fn default_notify_props() -> BTreeMap<String, String> {
    [
        ("email", "true"),
        ("push", "mention"),
        ("desktop", "mention"),
        ("desktop_sound", "true"),
        ("mention_keys", ""),
        ("channel", "true"),
        ("push_status", "online"),
        ("comments", "never"),
        ("first_name", "false"),
        ("desktop_threads", "all"),
        ("email_threads", "all"),
        ("push_threads", "all"),
        ("channel_mention_auto_follow_threads", "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn mention_keys(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|k| !k.is_empty()).map(str::to_string).collect()
}

/// `User.PreUpdate`.
fn pre_update(u: &mut User) {
    u.username = sanitize_unicode(&u.username).to_lowercase();
    u.first_name = sanitize_unicode(&u.first_name);
    u.last_name = sanitize_unicode(&u.last_name);
    u.nickname = sanitize_unicode(&u.nickname);
    u.email = u.email.to_lowercase();
    u.update_at = now_ms();
    if u.auth_data.as_deref() == Some("") {
        u.auth_data = None;
    }
    if u.notify_props.0.is_empty() {
        u.notify_props.0 = default_notify_props();
    } else if let Some(k) = u.notify_props.0.get("mention_keys").cloned() {
        u.notify_props.0.insert("mention_keys".into(), mention_keys(&k).join(","));
    }
    if let Some(mut cs) = custom_status_of(u) {
        cs.pre_save();
        set_custom_status_prop(u, &cs);
    }
}

/// `CheckEmailDomain`: one of the space-separated domains, or any when none.
fn email_domain_ok(email: &str, domains: &str) -> bool {
    let d: Vec<String> = domains.to_lowercase().replace(['@', ','], " ").split_whitespace().map(str::to_string).collect();
    d.is_empty() || d.iter().any(|d| email.to_lowercase().ends_with(&format!("@{d}")))
}

/// `IsPasswordValidWithSettings`.
pub fn check_password(app: &App, p: &str) -> ApiResult<()> {
    let min = app.cfg.i64("PasswordSettings.MinimumLength") as usize;
    let mut id = "model.user.is_valid.pwd".to_string();
    let mut bad = false;
    let mut minmax = false;
    if p.len() < min {
        (bad, minmax) = (true, true);
        id += "_min_length";
    }
    if p.len() > 72 {
        (bad, minmax) = (true, true);
        id += "_max_length";
    }
    if !minmax {
        let need = |k: &str| app.cfg.bool(&format!("PasswordSettings.{k}"));
        if need("Lowercase") && !p.chars().any(|c| c.is_ascii_lowercase()) {
            bad = true;
            id += "_lowercase";
        }
        if need("Uppercase") && !p.chars().any(|c| c.is_ascii_uppercase()) {
            bad = true;
            id += "_uppercase";
        }
        if need("Number") && !p.chars().any(|c| c.is_ascii_digit()) {
            bad = true;
            id += "_number";
        }
        if need("Symbol") && !p.chars().any(|c| " !\"\\#$%&'()*+,-./:;<=>?@[]^_`|~".contains(c)) {
            bad = true;
            id += "_symbol";
        }
    }
    if !bad {
        return Ok(());
    }
    id += ".app_error";
    let min = min.to_string();
    let mut e = AppError::bad_request(&id, "Invalid password.");
    if let Some(m) = crate::i18n::tr(&id, &[("Min", &min)]) {
        e.message = m;
    }
    Err(e)
}

impl App {
    /// `UpdateUser` through `SqlUserStore.Update`: what the store keeps from the old row, uniqueness errors,
    /// caches, `user_updated` (with `notify`).
    pub async fn update_user(&self, mut user: User, notify: bool, trusted: bool) -> ApiResult<User> {
        let old = self.user(&user.id).await?.ok_or_else(missing_account)?;
        let old = (*old).clone();
        pre_update(&mut user);
        user.create_at = old.create_at;
        check_user(&user)?;
        if user.email != old.email {
            let restrict = self.cfg.str("TeamSettings.RestrictCreationToDomains");
            if !email_domain_ok(&user.email, restrict) && !old.is_guest() {
                return Err(AppError::bad_request("api.user.update_user.accepted_domain.app_error", "Email is not from an accepted domain."));
            }
        }
        user.auth_data = old.auth_data.clone();
        user.auth_service = old.auth_service.clone();
        user.password = old.password.clone();
        user.last_password_update = old.last_password_update;
        user.last_picture_update = old.last_picture_update;
        user.email_verified = old.email_verified;
        user.failed_attempts = old.failed_attempts;
        user.mfa_secret = old.mfa_secret.clone();
        user.mfa_active = old.mfa_active;
        user.mfa_used_timestamps = old.mfa_used_timestamps.clone();
        user.last_login = old.last_login;
        user.remote_id = old.remote_id.clone();
        if !trusted {
            user.roles = old.roles.clone();
            user.delete_at = old.delete_at;
            if user.email != old.email {
                user.email_verified = false;
            }
        }
        if user.username != old.username {
            // UpdateMentionKeysFromUsername
            let keys: Vec<String> = mention_keys(user.notify_prop("mention_keys"))
                .into_iter()
                .filter(|k| *k != old.username && *k != format!("@{}", old.username))
                .collect();
            let v = if keys.is_empty() { String::new() } else { format!(",{}", keys.join(",")) };
            user.notify_props.0.insert("mention_keys".into(), v);
        }
        let r = sqlx::query(
            "UPDATE Users SET CreateAt=$2, UpdateAt=$3, DeleteAt=$4, Username=$5, Password=$6, AuthData=$7,
                 AuthService=$8, Email=$9, EmailVerified=$10, Nickname=$11, FirstName=$12, LastName=$13, Position=$14,
                 Roles=$15, AllowMarketing=$16, Props=$17, NotifyProps=$18, LastPasswordUpdate=$19,
                 LastPictureUpdate=$20, FailedAttempts=$21, Locale=$22, Timezone=$23, MfaActive=$24, MfaSecret=$25,
                 RemoteId=$26, LastLogin=$27, MfaUsedTimestamps=$28
             WHERE Id=$1",
        )
        .bind(&user.id)
        .bind(user.create_at)
        .bind(user.update_at)
        .bind(user.delete_at)
        .bind(&user.username)
        .bind(&user.password)
        .bind(&user.auth_data)
        .bind(&user.auth_service)
        .bind(&user.email)
        .bind(user.email_verified)
        .bind(&user.nickname)
        .bind(&user.first_name)
        .bind(&user.last_name)
        .bind(&user.position)
        .bind(&user.roles)
        .bind(user.allow_marketing)
        .bind(&user.props)
        .bind(&user.notify_props)
        .bind(user.last_password_update)
        .bind(user.last_picture_update)
        .bind(user.failed_attempts)
        .bind(&user.locale)
        .bind(&user.timezone)
        .bind(user.mfa_active)
        .bind(&user.mfa_secret)
        .bind(&user.remote_id)
        .bind(user.last_login)
        .bind(&user.mfa_used_timestamps)
        .execute(&self.db)
        .await;
        if let Err(e) = r {
            let m = e.to_string();
            if m.contains("username") {
                return Err(AppError::bad_request("app.user.save.username_exists.app_error", "An account with that username already exists."));
            }
            if m.contains("email") {
                return Err(AppError::bad_request("app.user.save.email_exists.app_error", "An account with that email already exists."));
            }
            return Err(AppError::internal("app.user.update.finding.app_error", m));
        }
        self.invalidate_user(&user.id);
        if user.username != old.username && user.last_picture_update <= 0 {
            let _ = self.update_default_profile_image(&user).await;
        }
        let mut fresh = self.user(&user.id).await?.map(|u| (*u).clone()).unwrap_or(user);
        if notify {
            self.send_updated_user_event(&fresh);
        }
        fresh.sanitize_own();
        Ok(fresh)
    }

    /// `sendUpdatedUserEvent`: admins get the full profile, others the sanitized one, the user their own.
    pub fn send_updated_user_event(&self, u: &User) {
        let mut omit = BTreeMap::new();
        omit.insert(u.id.clone(), true);
        let mut admin = u.clone();
        admin.sanitize_profile(Show { email: true, fullname: true, admin: true });
        let mut user = u.clone();
        user.sanitize_profile(Show {
            email: self.cfg.bool("PrivacySettings.ShowEmailAddress"),
            fullname: self.cfg.bool("PrivacySettings.ShowFullName"),
            admin: false,
        });
        self.hub.publish(
            "user_updated",
            json!({ "user": admin }),
            Broadcast { omit_users: Some(omit), sanitized: Some(json!({ "user": user })), ..Default::default() },
        );
        let mut own = u.clone();
        own.sanitize_own();
        self.hub.publish("user_updated", json!({ "user": own }), Broadcast { user_id: u.id.clone(), ..Default::default() });
    }

    /// `DoubleCheckPassword`: a password attempt that counts toward the lockout.
    pub async fn double_check_password(&self, u: &User, password: &str) -> ApiResult<()> {
        let max = self.cfg.i64("ServiceSettings.MaximumLoginAttempts") as i32;
        let claimed = sqlx::query("UPDATE Users SET FailedAttempts = FailedAttempts + 1 WHERE Id = $1 AND FailedAttempts < $2")
            .bind(&u.id)
            .bind(max)
            .execute(&self.db)
            .await?
            .rows_affected()
            == 1;
        self.invalidate_user(&u.id);
        if !claimed {
            return Err(AppError::new(StatusCode::UNAUTHORIZED, "api.user.check_user_login_attempts.too_many.app_error", ""));
        }
        match password::check(&u.password, password) {
            password::Check::Mismatch => {
                return Err(AppError::new(StatusCode::UNAUTHORIZED, "api.user.check_user_password.invalid.app_error", ""));
            }
            password::Check::MatchOld => self.store_password(&u.id, &password::hash(password)).await?,
            password::Check::Match => {}
        }
        sqlx::query("UPDATE Users SET FailedAttempts = 0 WHERE Id = $1").bind(&u.id).execute(&self.db).await?;
        self.invalidate_user(&u.id);
        Ok(())
    }

    /// `SqlUserStore.UpdatePassword`.
    pub async fn store_password(&self, user_id: &str, hash: &str) -> ApiResult<()> {
        let now = now_ms();
        sqlx::query(
            "UPDATE Users SET Password = $1, LastPasswordUpdate = $2, UpdateAt = $2, AuthData = NULL,
                 AuthService = '', FailedAttempts = 0 WHERE Id = $3",
        )
        .bind(hash)
        .bind(now)
        .bind(user_id)
        .execute(&self.db)
        .await?;
        self.invalidate_user(user_id);
        Ok(())
    }

    /// `UpdatePassword`: checked, stored, and with `TerminateSessionsOnPasswordChange` the other sessions revoked.
    pub async fn set_password(&self, u: &User, new: &str, current_session: Option<&Session>) -> ApiResult<()> {
        check_password(self, new)?;
        if u.remote_id.as_deref().is_some_and(|r| !r.is_empty()) {
            return Err(AppError::internal("api.user.update_password.failed.app_error", "remote user"));
        }
        self.store_password(&u.id, &password::hash(new)).await?;
        if self.cfg.bool("ServiceSettings.TerminateSessionsOnPasswordChange") {
            let keep = current_session.filter(|s| s.user_id == u.id).map(|s| s.id.clone()).unwrap_or_default();
            let sessions: Vec<(String, String)> =
                sqlx::query_as("SELECT Id, Token FROM Sessions WHERE UserId = $1 AND Id <> $2")
                    .bind(&u.id)
                    .bind(&keep)
                    .fetch_all(&self.db)
                    .await?;
            for (id, token) in sessions {
                self.revoke_session(&Session { id, token, ..Default::default() }).await?;
            }
        }
        Ok(())
    }
}

// ---------- patch ----------

/// `model.UserPatch`: absent fields stay as they are.
#[derive(Deserialize, Default)]
#[serde(default)]
struct UserPatch {
    username: Option<String>,
    password: Option<String>,
    nickname: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    position: Option<String>,
    email: Option<String>,
    props: Option<BTreeMap<String, String>>,
    notify_props: Option<BTreeMap<String, String>>,
    locale: Option<String>,
    timezone: Option<BTreeMap<String, String>>,
}

/// `patchUser`.
async fn patch_user(
    St(app): St<State>,
    Auth(s): Auth,
    Path(user_id): Path<String>,
    Body(patch): Body<UserPatch>,
) -> ApiResult<Json<User>> {
    let user_id = user_param(&s, &user_id)?;
    if !user_or_bot_perm(&app, &s, &user_id).await? {
        return Err(AppError::forbidden_perm(perm::EDIT_OTHER_USERS));
    }
    let old = app.user(&user_id).await?.ok_or_else(|| AppError::invalid_param("user_id"))?;
    if old.is_system_admin() && !app.has_perm(&s, perm::MANAGE_SYSTEM).await {
        return Err(AppError::forbidden_perm(perm::MANAGE_SYSTEM));
    }
    if s.is_oauth && patch.email.as_ref().is_some_and(|e| *e != old.email) {
        return Err(AppError::forbidden_perm(perm::EDIT_OTHER_USERS));
    }
    if let Some(e) = &patch.email
        && *e != old.email
        && s.user_id == user_id
    {
        let Some(p) = &patch.password else { return Err(AppError::invalid_param("password")) };
        app.double_check_password(&old, p).await?;
    }
    let mut u = (*old).clone();
    if let Some(v) = patch.username {
        u.username = v;
    }
    if let Some(v) = patch.nickname {
        u.nickname = v;
    }
    if let Some(v) = patch.first_name {
        u.first_name = v;
    }
    if let Some(v) = patch.last_name {
        u.last_name = v;
    }
    if let Some(v) = patch.position {
        u.position = v;
    }
    if let Some(v) = patch.email {
        u.email = v;
    }
    if let Some(v) = patch.props {
        u.props.0 = v;
    }
    if let Some(v) = patch.notify_props {
        u.notify_props.0 = v;
    }
    if let Some(v) = patch.locale {
        u.locale = v;
    }
    if let Some(v) = patch.timezone {
        u.timezone.0 = v;
    }
    let mut r = app.update_user(u, true, false).await?;
    // The API answers with the profile as the caller may see it.
    if r.id != s.user_id {
        r.sanitize_profile(app.show(&s).await);
    }
    Ok(Json(r))
}

/// `SessionHasPermissionToUserOrBot`: one's own bots too.
async fn user_or_bot_perm(app: &App, s: &Session, user_id: &str) -> ApiResult<bool> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT COALESCE(OwnerId, '') FROM Bots WHERE UserId = $1")
        .bind(user_id)
        .fetch_optional(&app.db)
        .await?;
    match owner {
        Some((owner,)) => Ok(owner == s.user_id && app.has_perm(s, "manage_bots").await
            || app.has_perm(s, "manage_others_bots").await),
        None => Ok(app.user_perm(s, user_id).await),
    }
}

// ---------- password ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct PasswordBody {
    current_password: String,
    new_password: String,
    already_hashed: String,
}

/// `updatePassword`.
async fn update_password(
    St(app): St<State>,
    Auth(s): Auth,
    Path(user_id): Path<String>,
    Body(b): Body<PasswordBody>,
) -> ApiResult<Json<Value>> {
    let user_id = user_param(&s, &user_id)?;
    let user = app.user(&user_id).await?;
    let can = match &user {
        Some(u) if u.is_system_admin() => app.has_perm(&s, perm::MANAGE_SYSTEM).await,
        Some(u) if u.is_bot => user_or_bot_perm(&app, &s, &user_id).await?,
        Some(_) => app.has_perm(&s, "sysconsole_write_user_management_users").await,
        None => false,
    };
    let context_err = || AppError::new(StatusCode::FORBIDDEN, "api.user.update_password.context.app_error", "");
    if b.already_hashed == "true" {
        if !can {
            return Err(if user_id == s.user_id {
                AppError::new(StatusCode::UNAUTHORIZED, "api.user.update_password.user_and_hashed.app_error", "")
            } else {
                context_err()
            });
        }
        let u = user.ok_or_else(missing_account)?;
        app.store_password(&u.id, &b.new_password).await?;
        return Ok(ok());
    }
    if user_id == s.user_id {
        if b.current_password.is_empty() {
            return Err(AppError::invalid_param("current_password"));
        }
        let u = user.ok_or_else(|| AppError::bad_request("api.user.update_password.valid_account.app_error", ""))?;
        if u.auth_data.as_deref().is_some_and(|a| !a.is_empty()) {
            return Err(AppError::bad_request("api.user.update_password.oauth.app_error", ""));
        }
        if let Err(e) = app.double_check_password(&u, &b.current_password).await {
            return Err(if e.id == "api.user.check_user_password.invalid.app_error" {
                AppError::bad_request("api.user.update_password.incorrect.app_error", "")
            } else {
                e
            });
        }
        let u = app.user(&user_id).await?.ok_or_else(missing_account)?;
        app.set_password(&u, &b.new_password, Some(&s)).await?;
    } else if can {
        let u = user.ok_or_else(missing_account)?;
        app.set_password(&u, &b.new_password, Some(&s)).await?;
    } else {
        return Err(context_err());
    }
    Ok(ok())
}

// ---------- custom status ----------

/// `model.CustomStatus`; `expires_at` is Go's `time.Time` in RFC 3339 (zero: `0001-01-01T00:00:00Z`).
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct CustomStatus {
    #[serde(default)]
    pub emoji: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub duration: String,
    #[serde(default = "zero_time")]
    pub expires_at: String,
}

fn zero_time() -> String {
    "0001-01-01T00:00:00Z".into()
}

impl CustomStatus {
    fn expires_ms(&self) -> Option<i64> {
        parse_rfc3339_ms(&self.expires_at)
    }
    fn is_zero_time(&self) -> bool {
        self.expires_at.starts_with("0001-01-01")
    }
    /// `CustomStatus.PreSave`.
    fn pre_save(&mut self) {
        if self.duration.is_empty() && !self.is_zero_time() && self.expires_ms().is_some_and(|t| t >= now_ms()) {
            self.duration = "date_and_time".into();
        }
        if self.text.chars().count() > 100 {
            self.text = self.text.chars().take(100).collect();
        }
    }
    /// `AreDurationAndExpirationTimeValid`.
    fn valid_times(&self) -> bool {
        let future = self.is_zero_time() || self.expires_ms().is_some_and(|t| t >= now_ms());
        if self.duration.is_empty() {
            return future;
        }
        matches!(
            self.duration.as_str(),
            "thirty_minutes" | "one_hour" | "four_hours" | "today" | "this_week" | "date_and_time"
        ) && !self.is_zero_time()
            && self.expires_ms().is_some_and(|t| t >= now_ms())
    }
}

/// RFC 3339 to unix ms (`2026-01-02T03:04:05.123Z`, `+03:00` offsets).
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let (time, offset) = if let Some(t) = rest.strip_suffix('Z') {
        (t, 0)
    } else {
        let i = rest.rfind(['+', '-'])?;
        let (t, o) = rest.split_at(i);
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let mut hm = o[1..].split(':');
        let oh: i64 = hm.next()?.parse().ok()?;
        let om: i64 = hm.next().unwrap_or("0").parse().ok()?;
        (t, sign * (oh * 3600 + om * 60))
    };
    let mut t = time.split(':');
    let (h, mi): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let sec: f64 = t.next()?.parse().ok()?;
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 - offset;
    Some(secs * 1000 + (sec * 1000.0).round() as i64)
}

fn custom_status_of(u: &User) -> Option<CustomStatus> {
    serde_json::from_str(u.props.0.get("customStatus")?).ok()
}

fn set_custom_status_prop(u: &mut User, cs: &CustomStatus) {
    u.props.0.insert("customStatus".into(), serde_json::to_string(cs).unwrap());
}

fn custom_status_disabled(app: &App) -> ApiResult<()> {
    if app.cfg.bool("TeamSettings.EnableCustomUserStatuses") {
        Ok(())
    } else {
        Err(AppError::new(StatusCode::NOT_IMPLEMENTED, "api.custom_status.disabled", "Custom statuses are disabled."))
    }
}

/// `updateUserCustomStatus` / `SetCustomStatus`.
async fn set_custom_status(
    St(app): St<State>,
    Auth(s): Auth,
    Path(user_id): Path<String>,
    Body(cs): Body<Option<CustomStatus>>,
) -> ApiResult<Json<Value>> {
    let user_id = user_param(&s, &user_id)?;
    custom_status_disabled(&app)?;
    let Some(mut cs) = cs.filter(|c| !(c.emoji.is_empty() && c.text.is_empty()) && c.valid_times()) else {
        return Err(AppError::invalid_param("custom_status"));
    };
    app.require_user(&s, &user_id).await?;
    cs.pre_save();
    if !cs.emoji.is_empty() && !app.emoji_exists(&cs.emoji).await? {
        return Err(AppError::bad_request("api.custom_status.set_custom_statuses.emoji_not_found", ""));
    }
    let mut u = (*app.user(&user_id).await?.ok_or_else(missing_account)?).clone();
    set_custom_status_prop(&mut u, &cs);
    app.update_user(u, true, false).await?;
    // addRecentCustomStatus: newest first, one per text, at most 5.
    let mut recent: Vec<CustomStatus> = match app.preference(&user_id, crate::preferences::CUSTOM_STATUS, "recent_custom_statuses").await? {
        Some(p) if !p.value.is_empty() => serde_json::from_str(&p.value).unwrap_or_default(),
        _ => vec![],
    };
    recent.retain(|r| r.text != cs.text);
    recent.insert(0, cs);
    recent.truncate(5);
    let pref = Preference {
        user_id: user_id.clone(),
        category: crate::preferences::CUSTOM_STATUS.into(),
        name: "recent_custom_statuses".into(),
        value: serde_json::to_string(&recent).unwrap(),
    };
    if let Err(e) = app.update_preferences(&user_id, vec![pref]).await {
        eprintln!("recent custom statuses of {user_id}: {}", e.message);
    }
    Ok(ok())
}

/// `removeUserCustomStatus`.
async fn remove_custom_status(St(app): St<State>, Auth(s): Auth, Path(user_id): Path<String>) -> ApiResult<Json<Value>> {
    let user_id = user_param(&s, &user_id)?;
    custom_status_disabled(&app)?;
    app.require_user(&s, &user_id).await?;
    let mut u = (*app.user(&user_id).await?.ok_or_else(missing_account)?).clone();
    u.props.0.insert("customStatus".into(), String::new());
    app.update_user(u, true, false).await?;
    Ok(ok())
}

/// `removeUserRecentCustomStatus`.
async fn remove_recent_custom_status(
    St(app): St<State>,
    Auth(s): Auth,
    Path(user_id): Path<String>,
    Body(cs): Body<CustomStatus>,
) -> ApiResult<Json<Value>> {
    let user_id = user_param(&s, &user_id)?;
    custom_status_disabled(&app)?;
    app.require_user(&s, &user_id).await?;
    let fail = || AppError::bad_request("api.custom_status.recent_custom_statuses.delete.app_error", "");
    let mut pref = app
        .preference(&user_id, crate::preferences::CUSTOM_STATUS, "recent_custom_statuses")
        .await?
        .ok_or_else(|| AppError::bad_request("app.preference.get.app_error", ""))?;
    if pref.value.is_empty() || cs.emoji.is_empty() && cs.text.is_empty() {
        return Err(fail());
    }
    let mut recent: Vec<CustomStatus> = serde_json::from_str(&pref.value).map_err(|_| fail())?;
    let before = recent.len();
    recent.retain(|r| *r != cs);
    if recent.len() == before {
        return Err(fail());
    }
    pref.value = serde_json::to_string(&recent).unwrap();
    app.update_preferences(&user_id, vec![pref]).await?;
    Ok(ok())
}

// ---------- sign-up ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct NewUser {
    username: String,
    password: String,
    email: String,
    nickname: String,
    first_name: String,
    last_name: String,
    position: String,
    locale: String,
    props: BTreeMap<String, String>,
    notify_props: BTreeMap<String, String>,
    timezone: Option<BTreeMap<String, String>>,
    email_verified: bool,
    auth_service: String,
    auth_data: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SignupQuery {
    t: String,
    iid: String,
}

/// `createUser`: with an invite id, as an admin, or by open sign-up.
async fn create_user(
    St(app): St<State>,
    MaybeAuth(s): MaybeAuth,
    Query(q): Query<SignupQuery>,
    Body(nu): Body<NewUser>,
) -> ApiResult<Response> {
    let admin = match &s {
        Some(s) => app.has_perm(s, perm::MANAGE_SYSTEM).await,
        None => false,
    };
    let signup_allowed = || -> ApiResult<()> {
        if !app.cfg.bool("EmailSettings.EnableSignUpWithEmail") || !app.cfg.bool("TeamSettings.EnableUserCreation") {
            return Err(AppError::new(StatusCode::NOT_IMPLEMENTED, "api.user.create_user.signup_email_disabled.app_error", ""));
        }
        Ok(())
    };
    let mut u = User {
        username: nu.username,
        password: nu.password,
        email: nu.email.trim().to_string(),
        nickname: nu.nickname,
        first_name: nu.first_name,
        last_name: nu.last_name,
        position: nu.position,
        locale: nu.locale,
        ..Default::default()
    };
    u.props.0 = nu.props;
    u.notify_props.0 = nu.notify_props;
    if let Some(tz) = nu.timezone {
        u.timezone.0 = tz;
    }
    if admin {
        u.email_verified = nu.email_verified;
        u.auth_service = nu.auth_service;
        u.auth_data = nu.auth_data;
    }
    let mut team = None;
    if !q.t.is_empty() {
        return Err(AppError::bad_request("api.user.create_user.signup_link_invalid.app_error", ""));
    } else if !q.iid.is_empty() {
        signup_allowed()?;
        let t = crate::teams::team_by_invite(&app, &q.iid).await?;
        if t.group_constrained == Some(true) {
            return Err(AppError::new(StatusCode::FORBIDDEN, "app.team.invite_id.group_constrained.error", ""));
        }
        if !email_domain_ok(&u.email, &t.allowed_domains) {
            let mut e = AppError::new(StatusCode::FORBIDDEN, "api.team.invite_members.invalid_email.app_error", "");
            if let Some(m) = crate::i18n::tr(&e.id, &[("Addresses", &t.allowed_domains)]) {
                e.message = m;
            }
            return Err(e);
        }
        u.email_verified = false;
        team = Some(t);
    } else if !admin {
        signup_allowed()?;
        let (users,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM Users").fetch_one(&app.db).await?;
        if users > 0 && !app.cfg.bool("TeamSettings.EnableOpenServer") {
            return Err(AppError::new(StatusCode::FORBIDDEN, "api.user.create_user.no_open_server", ""));
        }
        u.email_verified = false;
    }
    let ruser = app.create_user(u).await?;
    if let Some(t) = team {
        app.join_user_to_team(&t, &ruser, "").await?;
    }
    Ok((StatusCode::CREATED, Json(ruser)).into_response())
}

impl App {
    /// `createUserOrGuest` → `UserService.CreateUser` → `SqlUserStore.Save`, then the first preferences and
    /// `new_user`.
    pub async fn create_user(&self, mut u: User) -> ApiResult<User> {
        u.roles = "system_user".into();
        let restrict = self.cfg.str("TeamSettings.RestrictCreationToDomains");
        if u.auth_service.is_empty() && !email_domain_ok(&u.email, restrict) {
            return Err(AppError::bad_request("api.user.create_user.accepted_domain.app_error", ""));
        }
        let (humans,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM Users u LEFT JOIN Bots b ON b.UserId = u.Id WHERE b.UserId IS NULL")
            .fetch_one(&self.db)
            .await?;
        if humans == 0 {
            u.roles = "system_admin system_user".into();
        }
        if !LOCALES.contains(&u.locale.as_str()) {
            u.locale = self.cfg.str("LocalizationSettings.DefaultClientLocale").to_string();
        }
        if u.auth_service.is_empty() {
            check_password(self, &u.password)?;
        }
        // PreSave
        u.id = crate::app::new_id();
        if u.username.is_empty() {
            u.username = format!("a{}", crate::app::new_id());
        }
        if u.auth_data.as_deref() == Some("") {
            u.auth_data = None;
        }
        u.username = sanitize_unicode(&u.username).to_lowercase();
        u.first_name = sanitize_unicode(&u.first_name);
        u.last_name = sanitize_unicode(&u.last_name);
        u.nickname = sanitize_unicode(&u.nickname);
        u.email = u.email.to_lowercase();
        u.create_at = now_ms();
        u.update_at = u.create_at;
        u.last_password_update = u.create_at;
        u.mfa_active = false;
        // SanitizeInput: Go saves an empty remote id, not NULL.
        if u.remote_id.is_none() {
            u.remote_id = Some(String::new());
        }
        if u.locale.is_empty() {
            u.locale = "en".into();
        }
        if u.notify_props.0.is_empty() {
            u.notify_props.0 = default_notify_props();
        }
        if u.timezone.0.is_empty() {
            u.timezone.0 = [("automaticTimezone", ""), ("manualTimezone", ""), ("useAutomaticTimezone", "true")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        }
        if !u.password.is_empty() {
            u.password = password::hash(&u.password);
        }
        if let Some(mut cs) = custom_status_of(&u) {
            cs.pre_save();
            set_custom_status_prop(&mut u, &cs);
        }
        check_user(&u)?;
        let r = crate::db::insert(&self.db, &u).await;
        if let Err(e) = r {
            let m = e.to_string();
            if m.contains("email") {
                return Err(AppError::bad_request("app.user.save.email_exists.app_error", ""));
            }
            if m.contains("username") {
                return Err(AppError::bad_request("app.user.save.username_exists.app_error", ""));
            }
            return Err(AppError::internal("app.user.save.app_error", m));
        }
        self.invalidate_user(&u.id);
        let prefs = [
            ("recommended_next_steps", "hide", "false".to_string()),
            ("tutorial_step", u.id.as_str(), "0".to_string()),
            ("system_notice", "GMasDM", "true".to_string()),
        ];
        for (c, n, v) in prefs {
            let _ = sqlx::query(
                "INSERT INTO Preferences (UserId, Category, Name, Value) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (userid, category, name) DO UPDATE SET Value = $4",
            )
            .bind(&u.id)
            .bind(c)
            .bind(n)
            .bind(v)
            .execute(&self.db)
            .await;
        }
        self.hub.publish("new_user", json!({ "user_id": u.id }), Broadcast::default());
        let mut out = self.user(&u.id).await?.map(|x| (*x).clone()).unwrap_or(u);
        out.sanitize_own();
        Ok(out)
    }
}

// ---------- email ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct EmailBody {
    email: String,
}

/// `sendPasswordReset`: this server sends no email, so an existing account gets Go's sending error (hardened
/// mode answers OK, as Go does for every error).
async fn send_password_reset(St(app): St<State>, Body(b): Body<EmailBody>) -> ApiResult<Json<Value>> {
    let email = b.email.to_lowercase();
    if email.is_empty() {
        return Err(AppError::invalid_param("email"));
    }
    let hardened = app.cfg.bool("ServiceSettings.EnableHardenedMode");
    let user: Option<User> = crate::db::one(&app.db, "email = $1", &email).await?;
    let err = match user {
        None => return Ok(ok()),
        Some(u) if u.auth_data.as_deref().is_some_and(|a| !a.is_empty()) => {
            AppError::bad_request("api.user.send_password_reset.sso.app_error", "")
        }
        Some(_) => AppError::internal("api.user.send_password_reset.send.app_error", "email is not sent by this server"),
    };
    if hardened { Ok(ok()) } else { Err(err) }
}

/// `sendVerificationEmail`: always OK.
async fn send_verification(Body(b): Body<EmailBody>) -> ApiResult<Json<Value>> {
    if b.email.is_empty() {
        return Err(AppError::invalid_param("email"));
    }
    Ok(ok())
}

// ---------- sessions ----------

/// `getSessions`.
async fn get_sessions(St(app): St<State>, Auth(s): Auth, Path(user_id): Path<String>) -> ApiResult<Json<Vec<Session>>> {
    let user_id = user_param(&s, &user_id)?;
    app.require_user(&s, &user_id).await?;
    let sql = format!("{} WHERE userid = $1 ORDER BY lastactivityat DESC", Session::SELECT);
    let mut sessions: Vec<Session> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(&user_id).fetch_all(&app.db).await?;
    // SqlSessionStore.GetSessions: each with the user's team memberships.
    let members = app.session_team_members(&user_id).await?;
    for x in &mut sessions {
        x.sanitize();
        x.team_members = members.clone();
    }
    Ok(Json(sessions))
}

/// `handleDeviceProps`: the mobile app's push device and its props.
async fn device_props(St(app): St<State>, Auth(s): Auth, Body(props): Body<BTreeMap<String, Value>>) -> ApiResult<Json<Value>> {
    let get = |k: &str| props.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (device, voip) = (get("device_id"), get("voip_device_id"));
    let mut new_props = BTreeMap::new();
    let disabled = get("device_notification_disabled");
    if !disabled.is_empty() {
        if disabled != "true" && disabled != "false" {
            return Err(AppError::invalid_param("device_notification_disabled"));
        }
        new_props.insert("device_notification_disabled".to_string(), disabled);
    }
    let version = get("mobile_version");
    if !version.is_empty() {
        let semver = version.split('.').count() == 3 && version.split('.').all(|p| p.chars().next().is_some_and(|c| c.is_ascii_digit()));
        if !semver {
            return Err(AppError::invalid_param("mobile_version"));
        }
        new_props.insert("mobile_version".to_string(), version);
    }
    let mut session = (*s).clone();
    if !device.is_empty() || !voip.is_empty() {
        if !device.is_empty() {
            let others: Vec<(String, String)> =
                sqlx::query_as("SELECT Id, Token FROM Sessions WHERE UserId = $1 AND DeviceId = $2 AND Id <> $3")
                    .bind(&s.user_id)
                    .bind(&device)
                    .bind(&s.id)
                    .fetch_all(&app.db)
                    .await?;
            for (id, token) in others {
                app.revoke_session(&Session { id, token, ..Default::default() }).await?;
            }
            session.device_id = device;
        }
        if !voip.is_empty() {
            session.voip_device_id = voip;
        }
        session.expires_at = now_ms() + app.cfg.i64("ServiceSettings.SessionLengthMobileInHours") * 3_600_000;
        sqlx::query("UPDATE Sessions SET DeviceId = $1, VoIPDeviceId = $2, ExpiresAt = $3 WHERE Id = $4")
            .bind(&session.device_id)
            .bind(&session.voip_device_id)
            .bind(session.expires_at)
            .bind(&s.id)
            .execute(&app.db)
            .await?;
    }
    let mut changed = false;
    for (k, v) in new_props {
        if session.prop(&k) != v {
            session.props.0.insert(k, v);
            changed = true;
        }
    }
    if changed {
        sqlx::query("UPDATE Sessions SET Props = $1 WHERE Id = $2").bind(&session.props).bind(&s.id).execute(&app.db).await?;
    }
    app.clear_sessions_for_user(&s.user_id);
    Ok(ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(parse_rfc3339_ms("2021-01-01T03:00:00+03:00"), Some(1_609_459_200_000));
        assert_eq!(parse_rfc3339_ms("2021-01-01T00:00:00.5Z"), Some(1_609_459_200_500));
    }
    #[test]
    fn usernames() {
        assert!(valid_username("bench.01-x_y"));
        assert!(!valid_username("all"));
        assert!(!valid_username("Upper"));
    }
}

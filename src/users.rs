//! Users: profiles through the profile cache, login and logout, and the `/users` endpoints.

// Based on Mattermost server/channels/api4/user.go, app/user.go, app/login.go, app/authentication.go and
// store/sqlstore/user_store.go, Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::sync::Arc;

use axum::extract::{Path, Query, State as St};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::postgres::PgRow;

use crate::app::{ApiResult, App, AppError, Auth, Body, State, is_valid_id, new_id, now_ms};
use crate::db;
use crate::model::{Session, Show, User, session};
use crate::password;

pub fn routes() -> Router<State> {
    Router::new()
        .route("/api/v4/users/login", post(login))
        .route("/api/v4/users/logout", post(logout))
        .route("/api/v4/users/me", get(get_me))
        .route("/api/v4/users/ids", post(users_by_ids))
        .route("/api/v4/users/usernames", post(users_by_usernames))
        .route("/api/v4/users/{user_id}", get(get_user))
        .route("/api/v4/users/username/{username}", get(get_by_username))
        .route("/api/v4/users/email/{email}", get(get_by_email))
}

/// A user row with the bot columns of `usersQuery`.
struct WithBot(User);

impl<'r> sqlx::FromRow<'r, PgRow> for WithBot {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        let mut u = User::from_row(row)?;
        u.is_bot = row.try_get("isbot")?;
        u.bot_description = row.try_get("botdescription")?;
        u.bot_last_icon_update = row.try_get("botlasticonupdate")?;
        Ok(WithBot(u))
    }
}

/// `usersQuery`: users and their bot fields; `cond` on `u.`, one array or text bind.
pub async fn load_users(app: &App, cond: &str, arg: &[String]) -> ApiResult<Vec<User>> {
    let sql = format!(
        "SELECT {}, b.UserId IS NOT NULL AS isbot, COALESCE(b.Description, '') AS botdescription,
                COALESCE(b.LastIconUpdate, 0) AS botlasticonupdate
         FROM Users u LEFT JOIN Bots b ON b.UserId = u.Id WHERE {cond}",
        db::cols::<User>("u")
    );
    let rows: Vec<WithBot> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(arg).fetch_all(&app.db).await?;
    Ok(rows.into_iter().map(|w| w.0).collect())
}

async fn load_user_by(app: &App, cond: &str, arg: &str) -> ApiResult<Option<User>> {
    let sql = format!(
        "SELECT {}, b.UserId IS NOT NULL AS isbot, COALESCE(b.Description, '') AS botdescription,
                COALESCE(b.LastIconUpdate, 0) AS botlasticonupdate
         FROM Users u LEFT JOIN Bots b ON b.UserId = u.Id WHERE {cond}",
        db::cols::<User>("u")
    );
    let r: Option<WithBot> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(arg).fetch_optional(&app.db).await?;
    Ok(r.map(|w| w.0))
}

pub fn missing_account() -> AppError {
    AppError::not_found("app.user.missing_account.const", "Unable to find the user.")
}

impl App {
    /// `GetSanitizeOptions` for this viewer.
    pub async fn show(&self, s: &Session) -> Show {
        let admin = self.has_perm(s, crate::perm::MANAGE_SYSTEM).await;
        Show {
            email: admin || self.cfg.bool("PrivacySettings.ShowEmailAddress"),
            fullname: admin || self.cfg.bool("PrivacySettings.ShowFullName"),
            admin,
        }
    }

    /// A profile as `viewer` may see it: their own in full (`Sanitize({})`), others' sanitized.
    pub fn profile_for(&self, u: &User, viewer: &Session, show: Show) -> User {
        let mut u = u.clone();
        if u.id == viewer.user_id {
            u.sanitize_own();
        } else {
            u.sanitize_profile(show);
        }
        u
    }

    /// `InvalidateCacheForUser`: profile caches and channel memberships.
    pub fn invalidate_user(&self, user_id: &str) {
        self.caches.invalidate_user(user_id);
        self.caches.invalidate_members_for_user(user_id);
    }

    /// `invalidateUserCacheAndPublish`: `user_updated` to everyone, sanitized for non-admins.
    pub async fn publish_user_updated(&self, user_id: &str) {
        self.invalidate_user(user_id);
        let Ok(Some(u)) = self.user(user_id).await else { return };
        let mut admin = (*u).clone();
        admin.sanitize_profile(Show { email: true, fullname: true, admin: true });
        let mut user = (*u).clone();
        user.sanitize_profile(Show {
            email: self.cfg.bool("PrivacySettings.ShowEmailAddress"),
            fullname: self.cfg.bool("PrivacySettings.ShowFullName"),
            admin: false,
        });
        self.hub.publish(
            "user_updated",
            json!({ "user": admin }),
            crate::hub::Broadcast { sanitized: Some(json!({ "user": user })), ..Default::default() },
        );
    }
}

// ---------- reads ----------

/// `GetUserTermsOfService` on the user's own record (or for admins).
async fn with_terms(app: &App, u: &mut User) -> ApiResult<()> {
    let t: Option<(String, i64)> =
        sqlx::query_as("SELECT TermsOfServiceId, COALESCE(CreateAt, 0) FROM UserTermsOfService WHERE UserId = $1")
            .bind(&u.id)
            .fetch_optional(&app.db)
            .await?;
    if let Some((id, at)) = t {
        (u.terms_of_service_id, u.terms_of_service_create_at) = (id, at);
    }
    Ok(())
}

/// `getUser` (and `/users/me`): one's own record in full, others' as the privacy settings allow.
async fn one_user(app: &App, s: &Session, u: Arc<User>) -> ApiResult<Json<User>> {
    if !app.can_see_user(s, &u.id).await? {
        return Err(AppError::forbidden_perm(crate::perm::VIEW_MEMBERS));
    }
    let show = app.show(s).await;
    let mut u = (*u).clone();
    if show.admin || s.user_id == u.id {
        with_terms(app, &mut u).await?;
    }
    if u.id == s.user_id {
        u.sanitize_own();
    } else {
        u.sanitize_profile(show);
    }
    Ok(Json(u))
}

async fn get_me(St(app): St<State>, Auth(s): Auth) -> ApiResult<Json<User>> {
    let u = app.user(&s.user_id).await?.ok_or_else(missing_account)?;
    one_user(&app, &s, u).await
}

async fn get_user(St(app): St<State>, Auth(s): Auth, Path(id): Path<String>) -> ApiResult<Json<User>> {
    let id = s.resolve(&id);
    if !is_valid_id(&id) {
        return Err(AppError::invalid_url_param("user_id"));
    }
    let u = app.user(&id).await?.ok_or_else(missing_account)?;
    one_user(&app, &s, u).await
}

/// A missing user under view restrictions is a permission error (`getUserByUsername`, `getUserByEmail`).
async fn missing_or_restricted(app: &App, s: &Session, missing: AppError) -> AppError {
    match app.restriction_cond(s).await {
        Ok(Some(_)) => AppError::forbidden_perm(crate::perm::VIEW_MEMBERS),
        Ok(None) => missing,
        Err(e) => e,
    }
}

async fn get_by_username(St(app): St<State>, Auth(s): Auth, Path(name): Path<String>) -> ApiResult<Json<User>> {
    if !crate::user_edit::valid_username(&name) {
        return Err(AppError::invalid_param("username"));
    }
    let Some(u) = load_user_by(&app, "u.username = $1", &name).await? else {
        let missing = AppError::not_found("app.user.get_by_username.app_error", "Unable to find an existing account matching your username for this team. This team may require an invite from the team owner to join.");
        return Err(missing_or_restricted(&app, &s, missing).await);
    };
    one_user(&app, &s, Arc::new(u)).await
}

async fn get_by_email(St(app): St<State>, Auth(s): Auth, Path(email): Path<String>) -> ApiResult<Json<User>> {
    let email = email.to_lowercase();
    if !crate::user_edit::valid_email(&email) {
        return Err(AppError::invalid_url_param("email"));
    }
    let show = app.show(&s).await;
    if !show.email {
        return Err(AppError::forbidden().with_id("api.user.get_user_by_email.permissions.app_error"));
    }
    let Some(u) = load_user_by(&app, "u.email = $1", &email).await? else {
        return Err(missing_or_restricted(&app, &s, missing_account()).await);
    };
    if !app.can_see_user(&s, &u.id).await? {
        return Err(AppError::forbidden_perm(crate::perm::VIEW_MEMBERS));
    }
    let mut u = u;
    u.sanitize_profile(show);
    Ok(Json(u))
}

#[derive(Deserialize)]
struct SinceQuery {
    since: Option<String>,
}

/// `getUsersByIds`: the profile cache (`GetProfileByIds`), updated since `since`, every profile sanitized.
async fn users_by_ids(
    St(app): St<State>,
    Auth(s): Auth,
    Query(q): Query<SinceQuery>,
    Body(mut ids): Body<Vec<String>>,
) -> ApiResult<Json<Vec<User>>> {
    ids.sort();
    if ids.is_empty() {
        return Err(AppError::invalid_param("user_ids"));
    }
    let since = match q.since.as_deref() {
        None | Some("") => 0,
        Some(v) => v.parse::<i64>().map_err(|_| AppError::invalid_param("since"))?,
    };
    let mut users: Vec<Arc<User>> = match app.restriction_cond(&s).await? {
        None => app.users(&ids).await?,
        Some(r) => {
            let cond = format!("u.id = ANY($1) AND {r}");
            load_users(&app, &cond, &ids).await?.into_iter().map(Arc::new).collect()
        }
    };
    if since > 0 {
        users.retain(|u| u.update_at > since);
    }
    let show = app.show(&s).await;
    Ok(Json(users.iter().map(|u| sanitized(u, show)).collect()))
}

pub fn sanitized(u: &User, show: Show) -> User {
    let mut u = u.clone();
    u.sanitize_profile(show);
    u
}

async fn users_by_usernames(
    St(app): St<State>,
    Auth(s): Auth,
    Body(mut names): Body<Vec<String>>,
) -> ApiResult<Json<Vec<User>>> {
    names.sort();
    if names.is_empty() {
        return Err(AppError::invalid_param("usernames"));
    }
    let mut cond = "u.username = ANY($1)".to_string();
    if let Some(r) = app.restriction_cond(&s).await? {
        cond = format!("{cond} AND {r}");
    }
    let mut users = load_users(&app, &cond, &names).await?;
    users.sort_by(|a, b| a.username.cmp(&b.username));
    let show = app.show(&s).await;
    Ok(Json(users.iter().map(|u| sanitized(u, show)).collect()))
}

/// A bind of [`query_users`].
pub enum Arg {
    S(String),
    V(Vec<String>),
    I(i64),
}

/// `usersQuery` with joins, a condition and an order/limit, binds in order.
pub async fn query_users(app: &App, joins: &str, cond: &str, tail: &str, args: Vec<Arg>) -> ApiResult<Vec<User>> {
    let sql = format!(
        "SELECT {}, b.UserId IS NOT NULL AS isbot, COALESCE(b.Description, '') AS botdescription,
                COALESCE(b.LastIconUpdate, 0) AS botlasticonupdate
         FROM Users u LEFT JOIN Bots b ON b.UserId = u.Id {joins} WHERE {cond} {tail}",
        db::cols::<User>("u")
    );
    let mut q = sqlx::query_as::<_, WithBot>(sqlx::AssertSqlSafe(sql));
    for a in args {
        q = match a {
            Arg::S(v) => q.bind(v),
            Arg::V(v) => q.bind(v),
            Arg::I(v) => q.bind(v),
        };
    }
    Ok(q.fetch_all(&app.db).await?.into_iter().map(|w| w.0).collect())
}

// ---------- login ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct LoginBody {
    id: String,
    login_id: String,
    password: String,
    token: String,
    device_id: String,
}

fn login_error(app: &App, e: AppError) -> AppError {
    const UNMASKED: &[&str] = &[
        "mfa.validate_token.authenticate.app_error",
        "api.user.check_user_mfa.bad_code.app_error",
        "api.user.login.blank_pwd.app_error",
        "api.user.login.bot_login_forbidden.app_error",
        "api.user.login.inactive.app_error",
        "api.user.login.not_verified.app_error",
        "api.user.check_user_login_attempts.too_many.app_error",
    ];
    if UNMASKED.contains(&e.id.as_str()) {
        return e;
    }
    let (u, m) = (app.cfg.bool("EmailSettings.EnableSignInWithUsername"), app.cfg.bool("EmailSettings.EnableSignInWithEmail"));
    let id = match (u, m) {
        (true, false) => "api.user.login.invalid_credentials_username",
        (false, true) => "api.user.login.invalid_credentials_email",
        _ => "api.user.login.invalid_credentials_email_username",
    };
    AppError::new(StatusCode::UNAUTHORIZED, id, "Enter a valid email or username and/or password.")
}

async fn login(St(app): St<State>, headers: HeaderMap, Body(b): Body<LoginBody>) -> Response {
    match do_login(&app, &headers, b).await {
        Ok(r) => r,
        Err(e) => login_error(&app, e).into_response(),
    }
}

/// `AuthenticateUserForLogin` + `DoLogin` for email/username and password.
async fn do_login(app: &State, headers: &HeaderMap, b: LoginBody) -> ApiResult<Response> {
    if b.password.is_empty() {
        return Err(AppError::bad_request("api.user.login.blank_pwd.app_error", "Password field must not be blank"));
    }
    let not_found = || AppError::bad_request("store.sql_user.get_for_login.app_error", "Unable to find an existing account matching your credentials.");
    let (by_name, by_email) = (app.cfg.bool("EmailSettings.EnableSignInWithUsername"), app.cfg.bool("EmailSettings.EnableSignInWithEmail"));
    if !by_name && !by_email {
        return Err(not_found());
    }
    let user = if !b.id.is_empty() {
        app.user(&b.id).await?.map(|u| (*u).clone())
    } else {
        let cond = match (by_name, by_email) {
            (true, true) => "(lower(u.username) = lower($1) OR lower(u.email) = lower($1))",
            (true, false) => "lower(u.username) = lower($1)",
            _ => "lower(u.email) = lower($1)",
        };
        load_user_by(app, cond, &b.login_id).await?
    };
    let user = user.ok_or_else(not_found)?;
    let unauthorized = |id: &str, m: &str| AppError::new(StatusCode::UNAUTHORIZED, id, m);
    if user.delete_at > 0 {
        return Err(unauthorized("api.user.login.inactive.app_error", "Login failed because your account has been deactivated."));
    }
    if user.is_bot {
        return Err(unauthorized("api.user.login.bot_login_forbidden.app_error", "Bot login is forbidden."));
    }
    if !user.auth_service.is_empty() {
        return Err(AppError::bad_request("api.user.login.use_auth_service.app_error", "Please log in using the account's sign-in method."));
    }
    let max = app.cfg.i64("ServiceSettings.MaximumLoginAttempts");
    let too_many = || unauthorized("api.user.check_user_login_attempts.too_many.app_error", "Your account is locked because of too many failed password attempts. Please reset your password.");
    if user.failed_attempts as i64 >= max {
        return Err(too_many());
    }
    // TryIncrementFailedPasswordAttempts: the slot is refunded or cleared below.
    let claimed = sqlx::query("UPDATE Users SET FailedAttempts = FailedAttempts + 1 WHERE Id = $1 AND FailedAttempts < $2")
        .bind(&user.id)
        .bind(max as i32)
        .execute(&app.db)
        .await?
        .rows_affected()
        == 1;
    app.invalidate_user(&user.id);
    if !claimed {
        return Err(too_many());
    }
    let refund = || async {
        let _ = sqlx::query("UPDATE Users SET FailedAttempts = FailedAttempts - 1 WHERE Id = $1 AND FailedAttempts > 0")
            .bind(&user.id)
            .execute(&app.db)
            .await;
        app.invalidate_user(&user.id);
    };
    match password::check(&user.password, &b.password) {
        password::Check::Mismatch => {
            return Err(unauthorized("api.user.check_user_password.invalid.app_error", "Login failed because of invalid password"));
        }
        password::Check::MatchOld => {
            sqlx::query("UPDATE Users SET Password = $1, UpdateAt = $2, AuthData = NULL, AuthService = '', FailedAttempts = 0 WHERE Id = $3")
                .bind(password::hash(&b.password))
                .bind(now_ms())
                .bind(&user.id)
                .execute(&app.db)
                .await?;
        }
        password::Check::Match => {}
    }
    if user.mfa_active && app.cfg.bool("ServiceSettings.EnableMultifactorAuthentication") {
        if b.token.is_empty() {
            refund().await;
            return Err(AppError::bad_request("mfa.validate_token.authenticate.app_error", "Invalid MFA token."));
        }
        if !password::totp_ok(&user.mfa_secret, &b.token, now_ms() / 1000) {
            return Err(unauthorized("api.user.check_user_mfa.bad_code.app_error", "Invalid MFA token."));
        }
    }
    sqlx::query("UPDATE Users SET FailedAttempts = 0 WHERE Id = $1").bind(&user.id).execute(&app.db).await?;
    app.invalidate_user(&user.id);
    if !user.email_verified && app.cfg.bool("EmailSettings.RequireEmailVerification") {
        return Err(unauthorized("api.user.login.not_verified.app_error", "Login failed because email address has not been verified."));
    }

    // DoLogin
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let mobile = !b.device_id.is_empty() || crate::user_agent::is_mobile(ua);
    let mut s = Session { user_id: user.id.clone(), roles: user.roles.clone(), device_id: b.device_id.clone(), ..Default::default() };
    let p = &mut s.props.0;
    p.insert("isMobile".into(), mobile.to_string());
    p.insert("isSaml".into(), "false".into());
    p.insert("isOAuthUser".into(), "false".into());
    p.insert(session::PROP_CSRF.into(), new_id());
    let agent = crate::user_agent::parse(ua);
    p.insert("platform".into(), agent.platform.into());
    p.insert("os".into(), agent.os.into());
    p.insert("browser".into(), agent.browser);
    p.insert("is_guest".into(), user.is_guest().to_string());
    let hours = if mobile {
        app.cfg.i64("ServiceSettings.SessionLengthMobileInHours")
    } else {
        app.cfg.i64("ServiceSettings.SessionLengthWebInHours")
    };
    s.token = new_id();
    s.expires_at = now_ms() + hours * 3_600_000;
    if !b.device_id.is_empty() {
        // RevokeOtherSessionsForDeviceId
        let old: Vec<(String, String)> =
            sqlx::query_as("SELECT Id, Token FROM Sessions WHERE UserId = $1 AND DeviceId = $2")
                .bind(&user.id)
                .bind(&b.device_id)
                .fetch_all(&app.db)
                .await?;
        for (id, token) in old {
            let _ = app.revoke_session(&Session { id, token, ..Default::default() }).await;
        }
    }
    app.save_session(&mut s).await?;
    sqlx::query("UPDATE Users SET LastLogin = $1 WHERE Id = $2").bind(s.create_at).bind(&user.id).execute(&app.db).await?;
    app.invalidate_user(&user.id);

    let mut out = app.user(&user.id).await?.map(|u| (*u).clone()).unwrap_or(user);
    out.sanitize_own();
    let mut r = (StatusCode::OK, Json(out)).into_response();
    let h = r.headers_mut();
    h.insert("Token", HeaderValue::from_str(&s.token).unwrap());
    attach_cookies(app, h, &s, headers);
    Ok(r)
}

/// `AttachSessionCookies`.
fn attach_cookies(app: &App, h: &mut HeaderMap, s: &Session, req: &HeaderMap) {
    let max_age = app.cfg.i64("ServiceSettings.SessionLengthWebInHours") * 3600;
    let secure = req.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https")
        || app.cfg.str("ServiceSettings.SiteURL").starts_with("https://");
    let sec = if secure { "; Secure" } else { "" };
    let exp = httpdate(now_ms() / 1000 + max_age);
    for (name, value, http_only) in [
        ("MMAUTHTOKEN", s.token.as_str(), "; HttpOnly"),
        ("MMUSERID", s.user_id.as_str(), ""),
        ("MMCSRF", s.prop(session::PROP_CSRF), ""),
    ] {
        let c = format!("{name}={value}; Path=/; Expires={exp}; Max-Age={max_age}{http_only}{sec}");
        h.append(header::SET_COOKIE, HeaderValue::from_str(&c).unwrap());
    }
}

/// RFC 1123 date for cookies.
fn httpdate(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    const WD: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WD[days.rem_euclid(7) as usize],
        d,
        MON[(m - 1) as usize],
        y,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Howard Hinnant's days → (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

async fn logout(St(app): St<State>, Auth(s): Auth) -> Response {
    let _ = app.revoke_session(&s).await;
    let mut r = Json(json!({"status": "OK"})).into_response();
    for name in ["MMAUTHTOKEN", "MMUSERID", "MMCSRF"] {
        let c = format!("{name}=; Path=/; Max-Age=0");
        r.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&c).unwrap());
    }
    r
}

pub fn ok() -> Json<Value> {
    Json(json!({"status": "OK"}))
}

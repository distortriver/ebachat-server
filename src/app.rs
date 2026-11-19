//! Server state, errors in Mattermost's shape, and the session behind a request.

// Based on Mattermost server/channels/app/session.go, web/handlers.go and model/utils.go (AppError),
// Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::FromRequestParts;
use axum::http::{Method, StatusCode, request::Parts};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sqlx::PgPool;

use crate::cache::Caches;
use crate::config::Config;
use crate::db::{self, Table};
use crate::files::FileStore;
use crate::hub::Hub;
use crate::model::{Session, User, session};

pub type State = Arc<App>;

pub struct App {
    pub db: PgPool,
    pub cfg: Config,
    pub caches: Caches,
    pub hub: Hub,
    pub files: FileStore,
    pub offline_queue: crate::status::OfflineQueue,
}

impl App {
    pub fn new(db: PgPool, cfg: Config) -> anyhow::Result<State> {
        let caches = Caches::new(cfg.i64("ServiceSettings.SessionCacheInMinutes").max(1) as u64);
        let files = FileStore::new(&cfg)?;
        DEVELOPER.store(cfg.bool("ServiceSettings.EnableDeveloper"), std::sync::atomic::Ordering::Relaxed);
        Ok(Arc::new(App { db, cfg, caches, hub: Hub::default(), files, offline_queue: Default::default() }))
    }

    /// `App.GetSession`: the session cache, the database (with the user's team memberships), or a personal
    /// access token.
    pub async fn session(&self, token: &str) -> ApiResult<Arc<Session>> {
        let s = match self.caches.session.get(token) {
            Some(s) => s,
            None => {
                let found: Option<Session> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "{} WHERE token = $1 OR id = $1 LIMIT 1",
                    Session::SELECT
                )))
                .bind(token)
                .fetch_optional(&self.db)
                .await?;
                let mut s = match found {
                    Some(s) => s,
                    None => self.session_for_access_token(token).await?,
                };
                if s.token != token {
                    return Err(invalid_token());
                }
                s.team_members = self.session_team_members(&s.user_id).await?;
                let s = Arc::new(s);
                if !s.is_expired(now_ms()) {
                    self.caches.session.put(token.to_string(), s.clone());
                }
                s
            }
        };
        if s.id.is_empty() || s.is_expired(now_ms()) {
            return Err(invalid_token());
        }
        let idle = self.cfg.i64("ServiceSettings.SessionIdleTimeoutInMinutes");
        if idle > 0
            && !s.is_oauth
            && !s.is_mobile_app()
            && !s.is_user_access_token()
            && !self.cfg.bool("ServiceSettings.ExtendSessionLengthWithActivity")
            && now_ms() - s.last_activity_at > idle * 60_000
        {
            let _ = self.revoke_session(&s).await;
            return Err(invalid_token());
        }
        Ok(s)
    }

    /// `GetTeamsForUser` without the deleted ones, for the session.
    pub async fn session_team_members(&self, user_id: &str) -> ApiResult<Vec<crate::model::TeamMember>> {
        crate::perm::team_members(self, "tm.userid = $1 AND tm.deleteat = 0", &[user_id]).await
    }

    /// `createSessionForUserAccessToken`: a session saved for the token, as Go does.
    async fn session_for_access_token(&self, token: &str) -> ApiResult<Session> {
        let t: Option<(String, String, bool, i64)> = sqlx::query_as(
            "SELECT Id, COALESCE(UserId, ''), COALESCE(IsActive, false), ExpiresAt FROM UserAccessTokens WHERE Token = $1",
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await?;
        let missing = || AppError::new(StatusCode::UNAUTHORIZED, "app.user_access_token.invalid_or_missing", "Invalid or missing token.");
        let Some((token_id, user_id, active, expires)) = t else { return Err(missing()) };
        if !active {
            return Err(missing());
        }
        let user = self.user(&user_id).await?.ok_or_else(missing)?;
        if !self.cfg.bool("ServiceSettings.EnableUserAccessTokens") && !user.is_bot {
            return Err(missing());
        }
        if expires > 0 && now_ms() > expires {
            return Err(AppError::new(StatusCode::UNAUTHORIZED, "app.user_access_token.expired", "Token expired."));
        }
        if user.delete_at != 0 {
            return Err(missing());
        }
        let mut s = Session { token: token.into(), user_id: user.id.clone(), roles: user.roles.clone(), ..Default::default() };
        let p = &mut s.props.0;
        p.insert("user_access_token_id".into(), token_id);
        p.insert(session::PROP_TYPE.into(), session::TYPE_USER_ACCESS_TOKEN.into());
        if user.is_bot {
            p.insert(session::PROP_IS_BOT.into(), "true".into());
        }
        p.insert("is_guest".into(), user.is_guest().to_string());
        s.expires_at = now_ms() + 100 * 365 * 24 * 3_600_000;
        if expires > 0 && expires < s.expires_at {
            s.expires_at = expires;
        }
        self.save_session(&mut s).await?;
        Ok(s)
    }

    /// `SqlSessionStore.Save`.
    pub async fn save_session(&self, s: &mut Session) -> ApiResult<()> {
        s.id = new_id();
        s.create_at = now_ms();
        if s.last_activity_at == 0 {
            s.last_activity_at = s.create_at;
        }
        db::insert(&self.db, s).await?;
        Ok(())
    }

    /// `RevokeSession`: deleted, out of the cache, and its websockets closed.
    pub async fn revoke_session(&self, s: &Session) -> ApiResult<()> {
        sqlx::query("DELETE FROM Sessions WHERE Id = $1").bind(&s.id).execute(&self.db).await?;
        self.caches.session.remove(&s.token);
        self.hub.close_session(&s.token);
        Ok(())
    }

    /// `ClearSessionCacheForUser`: the user's sessions are read again (roles, team memberships changed).
    pub fn clear_sessions_for_user(&self, user_id: &str) {
        self.caches.session.remove_where(|_, s| s.user_id == user_id);
    }

    /// `ExtendSessionExpiryIfNeeded`.
    pub async fn extend_session_if_needed(&self, s: &Session) {
        if !self.cfg.bool("ServiceSettings.ExtendSessionLengthWithActivity") || s.is_expired(now_ms()) {
            return;
        }
        let length = self.session_length(s);
        let threshold = ((length as f64 * 0.01).min(86_400_000.0) as i64).max(5 * 60_000);
        let now = now_ms();
        if now - (s.expires_at - length) < threshold {
            return;
        }
        let expiry = now + length;
        if sqlx::query("UPDATE Sessions SET ExpiresAt = $1 WHERE Id = $2")
            .bind(expiry)
            .bind(&s.id)
            .execute(&self.db)
            .await
            .is_ok()
        {
            let mut n = s.clone();
            n.expires_at = expiry;
            self.caches.session.put(n.token.clone(), Arc::new(n));
        }
    }

    /// `GetSessionLengthInMillis`.
    pub fn session_length(&self, s: &Session) -> i64 {
        if s.is_user_access_token() && s.expires_at > 0 {
            return (s.expires_at - now_ms()).max(0);
        }
        let hours = if s.is_mobile_app() {
            self.cfg.i64("ServiceSettings.SessionLengthMobileInHours")
        } else if s.is_sso_login() {
            self.cfg.i64("ServiceSettings.SessionLengthSSOInHours")
        } else {
            self.cfg.i64("ServiceSettings.SessionLengthWebInHours")
        };
        hours * 3_600_000
    }

    /// `UpdateLastActivityAtIfNeeded` (websocket activity).
    pub async fn touch_session(&self, s: &Session) {
        let now = now_ms();
        if now - s.last_activity_at < 5 * 60_000 {
            return;
        }
        let _ = sqlx::query("UPDATE Sessions SET LastActivityAt = $1 WHERE Id = $2")
            .bind(now)
            .bind(&s.id)
            .execute(&self.db)
            .await;
        let mut n = s.clone();
        n.last_activity_at = now;
        self.caches.session.put(n.token.clone(), Arc::new(n));
    }

    /// `UserStore.Get` through the profile cache, with the bot fields.
    pub async fn user(&self, id: &str) -> ApiResult<Option<Arc<User>>> {
        Ok(self.users(&[id.to_string()]).await?.into_iter().next())
    }

    /// `UserStore.GetProfileByIds` / `GetMany` through the profile cache: those found, in no order.
    pub async fn users(&self, ids: &[String]) -> ApiResult<Vec<Arc<User>>> {
        let mut out = Vec::with_capacity(ids.len());
        let mut missing: Vec<String> = vec![];
        for id in ids {
            match self.caches.user.get(id) {
                Some(u) => out.push(u),
                None if !missing.contains(id) => missing.push(id.clone()),
                None => {}
            }
        }
        if !missing.is_empty() {
            for u in crate::users::load_users(self, "u.id = ANY($1)", &missing).await? {
                let u = Arc::new(u);
                self.caches.user.put(u.id.clone(), u.clone());
                out.push(u);
            }
        }
        Ok(out)
    }

    /// `ChannelStore.Get` through the channel cache.
    pub async fn channel(&self, id: &str) -> ApiResult<Option<Arc<crate::model::Channel>>> {
        if let Some(c) = self.caches.channel.get(id) {
            return Ok(Some(c));
        }
        let c: Option<crate::model::Channel> = db::get(&self.db, id).await?;
        Ok(c.map(|c| {
            let c = Arc::new(c);
            self.caches.channel.put(id.to_string(), c.clone());
            c
        }))
    }

    /// The channel or 404 as `GetChannel` answers.
    pub async fn channel_or_404(&self, id: &str) -> ApiResult<Arc<crate::model::Channel>> {
        self.channel(id).await?.ok_or_else(|| {
            AppError::new(StatusCode::NOT_FOUND, "app.channel.get.existing.app_error", "Unable to find the existing channel.")
        })
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

/// `model.NewId`: 16 random bytes in base32 with Mattermost's alphabet, 26 characters.
pub fn new_id() -> String {
    const ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";
    let bytes: [u8; 16] = rand::random();
    let mut out = String::with_capacity(26);
    let (mut acc, mut bits) = (0u32, 0);
    for b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    out
}

/// `model.IsValidId`.
pub fn is_valid_id(s: &str) -> bool {
    s.len() == 26 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

// ---------- errors ----------

/// `model.AppError` as the API sends it.
#[derive(Debug, Serialize)]
pub struct AppError {
    pub id: String,
    pub message: String,
    pub detailed_error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub request_id: String,
    pub status_code: u16,
}

impl AppError {
    pub fn new(status: StatusCode, id: &str, message: &str) -> AppError {
        AppError {
            id: id.into(),
            message: crate::i18n::t(id).unwrap_or(message).into(),
            detailed_error: String::new(),
            request_id: new_id(),
            status_code: status.as_u16(),
        }
    }
    pub fn detail(mut self, d: impl Into<String>) -> AppError {
        self.detailed_error = d.into();
        self
    }
    /// The message with Go's `{{.Name}}` parameters filled in.
    pub fn with_params(mut self, args: &[(&str, &str)]) -> AppError {
        if let Some(m) = crate::i18n::tr(&self.id, args) {
            self.message = m;
        }
        self
    }
    pub fn with_param(self, k: &str, v: &str) -> AppError {
        self.with_params(&[(k, v)])
    }
    /// The same error under another id (and its message).
    pub fn with_id(mut self, id: &str) -> AppError {
        self.id = id.into();
        if let Some(m) = crate::i18n::t(id) {
            self.message = m.into();
        }
        self
    }
    /// `SetInvalidParam`.
    pub fn invalid_param(name: &str) -> AppError {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "api.context.invalid_body_param.app_error",
            &format!("Invalid or missing {name} in request body."),
        )
    }
    /// `SetInvalidURLParam`.
    pub fn invalid_url_param(name: &str) -> AppError {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "api.context.invalid_url_param.app_error",
            &format!("Invalid or missing {name} parameter in request URL."),
        )
    }
    pub fn forbidden() -> AppError {
        AppError::new(
            StatusCode::FORBIDDEN,
            "api.context.permissions.app_error",
            "You do not have the appropriate permissions.",
        )
    }
    /// `SetPermissionError`.
    pub fn forbidden_perm(p: &str) -> AppError {
        AppError::forbidden().detail(format!("userId=, permission={p}"))
    }
    pub fn not_found(id: &str, message: &str) -> AppError {
        AppError::new(StatusCode::NOT_FOUND, id, message)
    }
    pub fn bad_request(id: &str, message: &str) -> AppError {
        AppError::new(StatusCode::BAD_REQUEST, id, message)
    }
    pub fn internal(id: &str, detail: impl Into<String>) -> AppError {
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, id, "An internal error occurred.").detail(detail)
    }
    pub fn not_implemented(what: &str) -> AppError {
        AppError::new(StatusCode::NOT_IMPLEMENTED, "api.not_implemented.app_error", &format!("{what} is not implemented."))
    }
}

pub fn invalid_token() -> AppError {
    AppError::new(StatusCode::UNAUTHORIZED, "api.context.session_expired.app_error", "Invalid or expired session, please login again.")
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> AppError {
        AppError::internal("app.sql.error", e.to_string())
    }
}

/// `ServiceSettings.EnableDeveloper`: errors keep `detailed_error` only then, as Go's `WipeDetailed`.
pub static DEVELOPER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl IntoResponse for AppError {
    fn into_response(mut self) -> Response {
        if self.status_code >= 500 {
            eprintln!("error {} {}: {}", self.status_code, self.id, self.detailed_error);
        }
        // Go blocks out the detail when not in developer mode.
        if !DEVELOPER.load(std::sync::atomic::Ordering::Relaxed) {
            self.detailed_error.clear();
        }
        let status = StatusCode::from_u16(self.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let rid = self.request_id.clone();
        let mut r = (status, axum::Json(self)).into_response();
        if let Ok(v) = rid.parse() {
            r.headers_mut().insert("X-Request-Id", v);
        }
        r
    }
}

pub type ApiResult<T> = Result<T, AppError>;

// ---------- the session of a request ----------

#[derive(PartialEq, Clone, Copy)]
pub enum TokenLocation {
    Header,
    Cookie,
    Query,
}

/// `ParseAuthTokenFromRequest`: `Authorization: Bearer|Token <t>`, the `MMAUTHTOKEN` cookie, or `?access_token=`.
pub fn token_from(parts: &Parts) -> Option<(String, TokenLocation)> {
    if let Some(h) = parts.headers.get("authorization").and_then(|v| v.to_str().ok())
        && let Some((kind, t)) = h.split_once(' ')
        && (kind.eq_ignore_ascii_case("bearer") || kind.eq_ignore_ascii_case("token"))
    {
        return Some((t.trim().to_string(), TokenLocation::Header));
    }
    for c in parts.headers.get_all("cookie") {
        for kv in c.to_str().unwrap_or("").split(';') {
            if let Some(t) = kv.trim().strip_prefix("MMAUTHTOKEN=") {
                return Some((t.to_string(), TokenLocation::Cookie));
            }
        }
    }
    let t = parts.uri.query()?.split('&').find_map(|kv| kv.strip_prefix("access_token=").map(str::to_string))?;
    Some((t, TokenLocation::Query))
}

/// A request with a valid session (`APISessionRequired`), CSRF-checked for cookie sessions.
pub struct Auth(pub Arc<Session>);

impl FromRequestParts<State> for Auth {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, app: &State) -> Result<Auth, AppError> {
        let (token, loc) = token_from(parts).ok_or_else(invalid_token)?;
        // web/handlers.go: any session error but a 500 is "session expired" for the client.
        let s = app.session(&token).await.map_err(|e| if e.status_code == 500 { e } else { invalid_token() })?;
        if loc == TokenLocation::Query && !s.is_oauth {
            return Err(AppError::new(
                StatusCode::UNAUTHORIZED,
                "api.context.token_provided.app_error",
                "Session is not OAuth but token was provided in the query string",
            ));
        }
        if loc == TokenLocation::Cookie && parts.method != Method::GET {
            let csrf = parts.headers.get("x-csrf-token").and_then(|v| v.to_str().ok()).unwrap_or("");
            let xhr = parts.headers.get("x-requested-with").and_then(|v| v.to_str().ok()) == Some("XMLHttpRequest");
            let strict = app.cfg.bool("ServiceSettings.ExperimentalStrictCSRFEnforcement");
            if !(csrf == s.prop(session::PROP_CSRF) && !csrf.is_empty() || xhr && !strict) {
                return Err(invalid_token());
            }
        }
        if s.user_id.is_empty()
            || (!app.cfg.bool("ServiceSettings.EnableUserAccessTokens")
                && s.is_user_access_token()
                && s.prop(session::PROP_IS_BOT) != "true")
        {
            return Err(invalid_token());
        }
        Ok(Auth(s))
    }
}

/// The session if there is a valid one (`APIHandler` endpoints).
pub struct MaybeAuth(pub Option<Arc<Session>>);

impl FromRequestParts<State> for MaybeAuth {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, app: &State) -> Result<MaybeAuth, Self::Rejection> {
        Ok(MaybeAuth(Auth::from_request_parts(parts, app).await.ok().map(|a| a.0)))
    }
}

impl Session {
    /// `me` in a path, or the id itself.
    pub fn resolve(&self, id: &str) -> String {
        if id == "me" { self.user_id.clone() } else { id.to_string() }
    }
}

/// A JSON request body read as Go reads it: whatever the `Content-Type`, a bad body is a 400 for `name`.
pub struct Body<T>(pub T);

impl<T: serde::de::DeserializeOwned> axum::extract::FromRequest<State> for Body<T> {
    type Rejection = AppError;
    async fn from_request(req: axum::extract::Request, state: &State) -> Result<Body<T>, AppError> {
        let bytes = axum::body::Bytes::from_request(req, state).await.map_err(|_| AppError::invalid_param("body"))?;
        let b: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) { b"null" } else { &bytes };
        serde_json::from_slice(b).map(Body).map_err(|e| AppError::invalid_param("body").detail(e.to_string()))
    }
}

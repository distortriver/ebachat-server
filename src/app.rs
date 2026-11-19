//! Server state, errors in Mattermost's shape, and the session behind a request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::FromRequestParts;
use axum::http::{StatusCode, request::Parts};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sqlx::PgPool;

use crate::db;
use crate::hub::Hub;
use crate::model::{Session, Status};

pub type State = Arc<App>;

pub struct App {
    pub db: PgPool,
    pub hub: Hub,
    sessions: Mutex<HashMap<String, (Arc<Session>, Instant)>>,
    pub statuses: RwLock<HashMap<String, Status>>,
}

/// How long a session read from the database is trusted before it is read again. Go keeps sessions in a cache
/// that other nodes invalidate; we have one node, so a revoke is seen within this time.
const SESSION_TTL: Duration = Duration::from_secs(60);

impl App {
    pub fn new(db: PgPool) -> State {
        Arc::new(App { db, hub: Hub::default(), sessions: Default::default(), statuses: Default::default() })
    }

    pub async fn session(&self, token: &str) -> Result<Arc<Session>, AppError> {
        if let Some((s, at)) = self.sessions.lock().unwrap().get(token)
            && at.elapsed() < SESSION_TTL
        {
            return Ok(s.clone());
        }
        let s: Option<Session> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{} WHERE token = $1", <Session as db::Table>::SELECT)))
                .bind(token)
                .fetch_optional(&self.db)
                .await?;
        let s = Arc::new(s.ok_or_else(invalid_session)?);
        self.sessions.lock().unwrap().insert(token.to_string(), (s.clone(), Instant::now()));
        Ok(s)
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
    pub request_id: String,
    pub status_code: u16,
}

impl AppError {
    pub fn new(status: StatusCode, id: &str, message: &str) -> AppError {
        AppError {
            id: id.into(),
            message: message.into(),
            detailed_error: String::new(),
            request_id: new_id(),
            status_code: status.as_u16(),
        }
    }
    pub fn invalid_param(name: &str) -> AppError {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "api.context.invalid_body_param.app_error",
            &format!("Invalid or missing {name} in request body."),
        )
    }
    pub fn forbidden() -> AppError {
        AppError::new(
            StatusCode::FORBIDDEN,
            "api.context.permissions.app_error",
            "You do not have the appropriate permissions.",
        )
    }
}

fn invalid_session() -> AppError {
    AppError::new(
        StatusCode::UNAUTHORIZED,
        "api.context.session_expired.app_error",
        "Invalid or expired session, please login again.",
    )
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> AppError {
        let mut a = AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "app.sql.error", "Database error.");
        a.detailed_error = e.to_string();
        a
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let rid = self.request_id.clone();
        let mut r = (status, axum::Json(self)).into_response();
        r.headers_mut().insert("X-Request-Id", rid.parse().unwrap());
        r
    }
}

pub type ApiResult<T> = Result<T, AppError>;

// ---------- the session of a request ----------

/// The token, as `ParseAuthTokenFromRequest` reads it: `Authorization: Bearer|Token <t>`, the `MMAUTHTOKEN`
/// cookie, or `?access_token=`.
pub fn token_from(parts: &Parts) -> Option<String> {
    if let Some(h) = parts.headers.get("authorization").and_then(|v| v.to_str().ok()) {
        let (kind, t) = h.split_once(' ')?;
        if kind.eq_ignore_ascii_case("bearer") || kind.eq_ignore_ascii_case("token") {
            return Some(t.trim().to_string());
        }
    }
    for c in parts.headers.get_all("cookie") {
        for kv in c.to_str().unwrap_or("").split(';') {
            if let Some(t) = kv.trim().strip_prefix("MMAUTHTOKEN=") {
                return Some(t.to_string());
            }
        }
    }
    parts.uri.query()?.split('&').find_map(|kv| kv.strip_prefix("access_token=").map(str::to_string))
}

/// A request with a valid session (`APISessionRequired`).
pub struct Auth(pub Arc<Session>);

impl FromRequestParts<State> for Auth {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, app: &State) -> Result<Auth, AppError> {
        let token = token_from(parts).ok_or_else(invalid_session)?;
        let s = app.session(&token).await?;
        if s.is_expired(now_ms()) {
            return Err(invalid_session());
        }
        Ok(Auth(s))
    }
}

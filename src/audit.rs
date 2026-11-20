//! The `Audits` table: the rows Go's handlers write with `c.LogAudit` and `c.LogAuditWithUserId`.

// Based on Mattermost server/channels/web/context.go (LogAudit, LogAuditWithUserId), web/handlers.go (the request's
// IP address and path), utils/utils.go (GetIPAddress) and store/sqlstore/audit_store.go, Copyright (c)
// 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::net::SocketAddr;

use axum::extract::ConnectInfo;

use crate::app::{App, new_id, now_ms};
use crate::model::Session;

/// What Go's request context knows of the request for an audit: the client's address and the URL path.
#[derive(Clone, Default)]
pub struct Req {
    pub ip: String,
    pub path: String,
    pub method: String,
    /// The session's user once the request's session is known (for the log fields).
    pub user_id: String,
}

#[derive(Default)]
struct ReqCell {
    req: Req,
    user_id: std::sync::OnceLock<String>,
}

tokio::task_local! {
    static REQ: std::sync::Arc<ReqCell>;
}

/// The request's address and path, set for the handler by `layer`.
pub fn req() -> Req {
    REQ.try_with(|c| {
        let mut r = c.req.clone();
        r.user_id = c.user_id.get().cloned().unwrap_or_default();
        r
    })
    .unwrap_or_default()
}

/// The session's user, once the request's session is read.
pub fn set_user(user_id: &str) {
    let _ = REQ.try_with(|c| c.user_id.set(user_id.to_string()));
}

/// `utils.GetIPAddress`: the first address of the first trusted proxy header that holds a valid IP, else the
/// peer's address.
pub fn ip_address(app: &App, headers: &axum::http::HeaderMap, peer: Option<SocketAddr>) -> String {
    if let Some(list) = app.cfg.get("ServiceSettings.TrustedProxyIPHeader").as_array() {
        for h in list.iter().filter_map(|h| h.as_str()) {
            let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()).filter(|v| !v.is_empty()) else { continue };
            let first = v.split(',').next().unwrap_or("").trim();
            if first.parse::<std::net::IpAddr>().is_ok() {
                return first.to_string();
            }
        }
    }
    peer.map(|p| p.ip().to_string()).unwrap_or_default()
}

/// The middleware that makes `req()` answer for the handlers of the request.
pub async fn layer(
    axum::extract::State(app): axum::extract::State<crate::app::State>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let r = Req {
        ip: ip_address(&app, req.headers(), peer),
        path: req.uri().path().to_string(),
        method: req.method().to_string(),
        user_id: String::new(),
    };
    REQ.scope(std::sync::Arc::new(ReqCell { req: r, user_id: Default::default() }), next.run(req)).await
}

/// `c.LogAudit`: a row for the session's user.
pub async fn log(app: &App, s: Option<&Session>, extra: &str) {
    let uid = s.map_or("", |s| s.user_id.as_str());
    save(app, s, uid, extra).await;
}

/// `c.LogAuditWithUserId`: a row for `user_id`, with the session's user (when there is one) in the extra info.
pub async fn log_user(app: &App, s: Option<&Session>, user_id: &str, extra: &str) {
    let mut extra = extra.to_string();
    if let Some(s) = s.filter(|s| !s.user_id.is_empty()) {
        extra = format!("{extra} session_user={}", s.user_id).trim().to_string();
    }
    save(app, s, user_id, &extra).await;
}

/// `SqlAuditStore.Save`; a failure is only logged, as Go does.
async fn save(app: &App, s: Option<&Session>, user_id: &str, extra: &str) {
    let r = req();
    let sid = s.map_or("", |s| s.id.as_str());
    if let Err(e) = sqlx::query(
        "INSERT INTO Audits (Id, CreateAt, UserId, Action, ExtraInfo, IpAddress, SessionId) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(new_id())
    .bind(now_ms())
    .bind(user_id)
    .bind(&r.path)
    .bind(extra)
    .bind(&r.ip)
    .bind(sid)
    .execute(&app.db)
    .await
    {
        crate::logs::error(
            "Failed to save audit",
            &[("err_where", "LogAudit".into()), ("error", format!("app.audit.save.saving.app_error: {e}").into())],
        );
    }
}

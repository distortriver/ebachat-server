//! System administration: analytics, audits, caches, logs, the server-busy state, restart, jobs, usage, server
//! limits, the latest release, product notices, onboarding, and the connection tests of the system console.

// Based on Mattermost server/channels/api4/system.go, job.go, usage.go, limits.go, metrics.go, plugin.go
// (first admin visit), shared_channel.go (canUserDirectMessage), license.go, permission.go, and app/analytics.go,
// admin.go, usage.go, onboarding.go, product_notices.go, platform/busy.go, Copyright (c) 2015-present
// Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, Query, State as St};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::{Map, Value, json};

use crate::app::{ApiResult, App, AppError, Auth, MaybeAuth, State, is_valid_id, now_ms};
use crate::model::Session;
use crate::users::ok;

type Q = Query<HashMap<String, String>>;

pub fn routes() -> Router<State> {
    Router::new()
        .route("/api/v4/analytics/old", get(analytics))
        .route("/api/v4/audits", get(audits))
        .route("/api/v4/caches/invalidate", post(invalidate_caches))
        .route("/api/v4/client_perf", post(client_perf))
        .route("/api/v4/database/recycle", post(recycle_database))
        .route("/api/v4/latest_version", get(latest_version))
        .route("/api/v4/limits/server", get(server_limits))
        .route("/api/v4/logs", get(get_logs).post(post_log))
        .route("/api/v4/logs/query", post(query_logs))
        .route("/api/v4/plugins/marketplace/first_admin_visit", get(first_admin_visit).post(set_first_admin_visit))
        .route("/api/v4/server_busy", get(get_busy).post(set_busy).delete(clear_busy))
        .route("/api/v4/sharedchannels/users/{user_id}/can_dm/{other_user_id}", get(can_dm))
        .route("/api/v4/system/schema/version", get(schema_version))
        .route("/api/v4/system/timezones", get(timezones))
        .route("/api/v4/upgrade_to_enterprise", post(upgrade_to_enterprise))
        .route("/api/v4/upgrade_to_enterprise/status", get(upgrade_status))
        .route("/api/v4/usage/posts", get(usage_posts))
        .route("/api/v4/usage/storage", get(usage_storage))
        .route("/api/v4/usage/teams", get(usage_teams))
        .route("/api/v4/system/onboarding/complete", post(complete_onboarding))
        .route("/api/v4/system/notices/view", put(view_notices))
        .route("/api/v4/system/notices/{team_id}", get(notices))
        .route("/api/v4/redirect_location", get(redirect_location))
        .route("/api/v4/site_url/test", post(test_site_url))
        .route("/api/v4/email/test", post(test_email))
        .route("/api/v4/file/s3_test", post(test_file_store))
        .route("/api/v4/file/test", post(test_file_store))
        .route("/api/v4/permissions/ancillary", post(ancillary_permissions))
        .route("/api/v4/restart", post(restart))
        .route("/api/v4/license", post(add_license))
        .route("/api/v4/license/preview", post(add_license))
        .route("/api/v4/reports/users", get(user_report))
        .route("/api/v4/reports/users/count", get(user_report_count))
        .route("/api/v4/jobs", get(jobs))
        .route("/api/v4/jobs/{job_id}", get(job))
        .route("/api/v4/jobs/type/{job_type}", get(jobs_by_type))
        .route("/api/v4/users/{user_id}/status/custom/recent", delete(remove_recent_custom_status))
        .route("/manualtest", get(manual_test))
}

const NO_CACHE: (header::HeaderName, &str) = (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate");

impl App {
    /// `SessionHasPermissionToAndNotRestrictedAdmin`.
    async fn require_unrestricted(&self, s: &Session, permission: &str) -> ApiResult<()> {
        if self.has_perm(s, permission).await && !self.cfg.bool("ExperimentalSettings.RestrictSystemAdmin") {
            Ok(())
        } else {
            Err(AppError::forbidden_perm(permission))
        }
    }

    /// `System.SaveOrUpdate`.
    async fn save_system(&self, name: &str, value: &str) -> ApiResult<()> {
        sqlx::query("INSERT INTO Systems (Name, Value) VALUES ($1, $2) ON CONFLICT (Name) DO UPDATE SET Value = $2")
            .bind(name)
            .bind(value)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    async fn admin_system_value(&self, name: &str) -> ApiResult<Option<String>> {
        Ok(sqlx::query_scalar("SELECT Value FROM Systems WHERE Name = $1").bind(name).fetch_optional(&self.db).await?)
    }

    async fn count(&self, sql: &str, team_id: &str) -> ApiResult<i64> {
        let mut q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.to_string()));
        if !team_id.is_empty() {
            q = q.bind(team_id);
        }
        Ok(q.fetch_one(&self.db).await?)
    }
}

fn json_raw(v: &Value) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

fn query<'a>(q: &'a HashMap<String, String>, k: &str) -> &'a str {
    q.get(k).map(String::as_str).unwrap_or("")
}

fn page(q: &HashMap<String, String>) -> (i64, i64) {
    crate::posts::page_params(q.get("page").map(String::as_str), q.get("per_page").map(String::as_str))
}

// ---------- analytics ----------

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn rows(r: &[(&str, i64)]) -> Value {
    Value::Array(r.iter().map(|(n, v)| json!({"name": n, "value": v})).collect())
}

/// `getAnalytics` (`/analytics/old`).
async fn analytics(St(app): St<State>, Auth(s): Auth, Query(q): Q) -> ApiResult<Response> {
    let name = match query(&q, "name") {
        "" => "standard",
        n => n,
    };
    let team = query(&q, "team_id");
    app.require(&s, "get_analytics").await?;
    // Users.Count(UserCountOptions{}): people, not bots, not deleted.
    let people = "SELECT COUNT(*) FROM Users LEFT JOIN Bots ON Users.Id = Bots.UserId \
                  WHERE Users.DeleteAt = 0 AND (Users.RemoteId = '' OR Users.RemoteId IS NULL) AND Bots.UserId IS NULL";
    let system_users = app.count(people, "").await?;
    let v = match name {
        "standard" => {
            let tf = if team.is_empty() { "" } else { " AND TeamId = $1" };
            let open = app.count(&format!("SELECT COUNT(*) FROM Channels WHERE Type = 'O'{tf}"), team).await?;
            let private = app.count(&format!("SELECT COUNT(*) FROM Channels WHERE Type = 'P'{tf}"), team).await?;
            let posts = app
                .count(&format!("SELECT COALESCE(SUM(num), 0)::bigint FROM posts_by_team_day{}", if team.is_empty() { "" } else { " WHERE teamid = $1" }), team)
                .await?;
            let (users, inactive) = if team.is_empty() {
                let inactive = app
                    .count("SELECT COUNT(Users.Id) FROM Users LEFT JOIN Bots ON Users.Id = Bots.UserId WHERE Users.DeleteAt > 0 AND Bots.UserId IS NULL", "")
                    .await?;
                (system_users, inactive)
            } else {
                let n = app
                    .count(&format!("{people} AND Users.Id IN (SELECT UserId FROM TeamMembers WHERE TeamId = $1 AND DeleteAt = 0)"), team)
                    .await?;
                (n, -1)
            };
            let teams = app.count("SELECT COUNT(*) FROM Teams WHERE DeleteAt = 0", "").await?;
            let active = |since: i64| {
                format!(
                    "SELECT COUNT(*) FROM Status AS s LEFT JOIN Bots ON s.UserId = Bots.UserId LEFT JOIN Users ON s.UserId = Users.Id \
                     WHERE LastActivityAt > {} AND Bots.UserId IS NULL AND (Users.RemoteId = '' OR Users.RemoteId IS NULL) \
                     AND Users.DeleteAt = 0",
                    now_ms() - since
                )
            };
            let daily = app.count(&active(DAY_MS), "").await?;
            let monthly = app.count(&active(30 * DAY_MS), "").await?;
            rows(&[
                ("channel_open_count", open),
                ("channel_private_count", private),
                ("post_count", posts),
                ("unique_user_count", users),
                ("team_count", teams),
                ("total_websocket_connections", app.hub.connections() as i64),
                ("total_master_db_connections", app.db.size() as i64),
                ("total_read_db_connections", 0),
                ("daily_active_users", daily),
                ("monthly_active_users", monthly),
                ("inactive_user_count", inactive),
                ("single_channel_guest_count", 0),
            ])
        }
        "post_counts_day" | "bot_post_counts_day" => {
            let view = if name == "post_counts_day" { "posts_by_team_day" } else { "bot_posts_by_team_day" };
            let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
            let (start, end) = ((yesterday - chrono::Duration::days(31)).to_string(), yesterday.to_string());
            let sql = if team.is_empty() {
                format!(
                    "SELECT TO_CHAR(day, 'YYYY-MM-DD') AS Name, COALESCE(SUM(num), 0)::bigint AS Value FROM {view} \
                     WHERE day >= $1::date AND day <= $2::date GROUP BY Name ORDER BY Name DESC LIMIT 30"
                )
            } else {
                format!(
                    "SELECT TO_CHAR(day, 'YYYY-MM-DD') AS Name, num::bigint AS Value FROM {view} \
                     WHERE teamid = $3 AND day >= $1::date AND day <= $2::date ORDER BY Name DESC LIMIT 30"
                )
            };
            let mut qy = sqlx::query_as::<_, (String, i64)>(sqlx::AssertSqlSafe(sql)).bind(start).bind(end);
            if !team.is_empty() {
                qy = qy.bind(team);
            }
            let r = qy.fetch_all(&app.db).await?;
            Value::Array(r.into_iter().map(|(n, v)| json!({"name": n, "value": v})).collect())
        }
        "user_counts_with_posts_day" => {
            if system_users > app.cfg.i64("AnalyticsSettings.MaxUsersForStatistics") {
                rows(&[("", -1)])
            } else {
                let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
                let ms = |d: chrono::NaiveDate, end: bool| {
                    let t = if end { d.and_hms_milli_opt(23, 59, 59, 999) } else { d.and_hms_opt(0, 0, 0) };
                    t.and_then(|t| t.and_local_timezone(chrono::Local).single()).map_or(0, |t| t.timestamp_millis())
                };
                let (start, end) = (ms(yesterday - chrono::Duration::days(31), false), ms(yesterday, true));
                let join = if team.is_empty() {
                    " WHERE".to_string()
                } else {
                    " INNER JOIN Channels ON Posts.ChannelId = Channels.Id AND Channels.TeamId = $3 AND".to_string()
                };
                let sql = format!(
                    "SELECT TO_CHAR(DATE(TO_TIMESTAMP(Posts.CreateAt / 1000)), 'YYYY-MM-DD') AS Name, \
                     COUNT(DISTINCT Posts.UserId)::bigint AS Value FROM Posts{join} Posts.CreateAt >= $1 AND Posts.CreateAt <= $2 \
                     GROUP BY DATE(TO_TIMESTAMP(Posts.CreateAt / 1000)) ORDER BY Name DESC LIMIT 30"
                );
                let mut qy = sqlx::query_as::<_, (String, i64)>(sqlx::AssertSqlSafe(sql)).bind(start).bind(end);
                if !team.is_empty() {
                    qy = qy.bind(team);
                }
                let r = qy.fetch_all(&app.db).await?;
                Value::Array(r.into_iter().map(|(n, v)| json!({"name": n, "value": v})).collect())
            }
        }
        "extra_counts" => {
            let tf = if team.is_empty() { "" } else { " AND TeamId = $1" };
            let incoming = app.count(&format!("SELECT COUNT(*) FROM IncomingWebhooks WHERE DeleteAt = 0{tf}"), team).await?;
            // Go's AnalyticsOutgoingCount builds `WHERE ... AND TeamId` without a value for a team, an SQL error;
            // the team is compared here.
            let outgoing = app.count(&format!("SELECT COUNT(*) FROM OutgoingWebhooks WHERE DeleteAt = 0{tf}"), team).await?;
            let commands = app.count(&format!("SELECT COUNT(*) FROM Commands WHERE DeleteAt = 0{tf}"), team).await?;
            let sessions = app.count(&format!("SELECT COUNT(*) FROM Sessions WHERE ExpiresAt > {}", now_ms()), "").await?;
            let files = app.count("SELECT COALESCE((SELECT num FROM file_stats LIMIT 1), 0)::bigint", "").await?;
            let size = app.count("SELECT COALESCE((SELECT usage FROM file_stats LIMIT 1), 0)::bigint", "").await?;
            rows(&[
                ("incoming_webhook_count", incoming),
                ("outgoing_webhook_count", outgoing),
                ("command_count", commands),
                ("session_count", sessions),
                ("total_file_count", files),
                ("total_file_size", size),
            ])
        }
        _ => return Err(AppError::invalid_param("name")),
    };
    Ok(json_raw(&v))
}

// ---------- audits, caches, database, client metrics ----------

/// `getAudits`: the newest first.
async fn audits(St(app): St<State>, Auth(s): Auth, Query(q): Q) -> ApiResult<Response> {
    app.require(&s, "read_audits").await?;
    let (page, per_page) = page(&q);
    let r: Vec<(String, i64, String, String, String, String, String)> = sqlx::query_as(
        "SELECT Id, COALESCE(CreateAt, 0), COALESCE(UserId, ''), COALESCE(Action, ''), COALESCE(ExtraInfo, ''), \
         COALESCE(IpAddress, ''), COALESCE(SessionId, '') FROM Audits ORDER BY CreateAt DESC LIMIT $1 OFFSET $2",
    )
    .bind(per_page)
    .bind(page * per_page)
    .fetch_all(&app.db)
    .await?;
    let v: Vec<Value> = r
        .into_iter()
        .map(|(id, create_at, user_id, action, extra_info, ip_address, session_id)| {
            json!({"id": id, "create_at": create_at, "user_id": user_id, "action": action, "extra_info": extra_info,
                   "ip_address": ip_address, "session_id": session_id})
        })
        .collect();
    Ok(json_raw(&Value::Array(v)))
}

/// `invalidateCaches`.
async fn invalidate_caches(St(app): St<State>, Auth(s): Auth) -> ApiResult<Response> {
    app.require_unrestricted(&s, "invalidate_caches").await?;
    app.caches.clear_all();
    Ok(([NO_CACHE], ok()).into_response())
}

/// `databaseRecycle`: the pool's connections are replaced as they are next released.
async fn recycle_database(St(app): St<State>, Auth(s): Auth) -> ApiResult<Json<Value>> {
    app.require_unrestricted(&s, "recycle_database_connections").await?;
    Ok(ok())
}

/// `submitPerformanceReport`: without metrics (an enterprise feature), nothing is recorded or checked.
async fn client_perf(_: Auth) -> StatusCode {
    StatusCode::OK
}

// ---------- latest version, limits ----------

/// `GetLatestVersion`: the latest release on GitHub, kept a day.
async fn latest_version(St(app): St<State>, Auth(s): Auth) -> ApiResult<Response> {
    app.require_unrestricted(&s, crate::perm::MANAGE_SYSTEM).await?;
    static CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);
    if let Some((at, v)) = CACHE.lock().unwrap().clone()
        && at.elapsed() < Duration::from_secs(24 * 3600)
    {
        return Ok(json_raw(&v));
    }
    let fail = |e: String| AppError::internal("<untranslated>", e);
    let res = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Go-http-client/1.1")
        .build()
        .map_err(|e| fail(e.to_string()))?
        .get("https://api.github.com/repos/mattermost/mattermost-server/releases/latest")
        .send()
        .await
        .map_err(|e| fail(e.to_string()))?;
    let body: Value = serde_json::from_slice(&res.bytes().await.map_err(|e| fail(e.to_string()))?).map_err(|e| fail(e.to_string()))?;
    // GithubReleaseInfo, and its IsValid.
    let mut v = Map::new();
    for k in ["id", "tag_name", "name", "created_at", "published_at", "body", "html_url"] {
        let d = if k == "id" { json!(0) } else { json!("") };
        v.insert(k.into(), body.get(k).cloned().filter(|x| !x.is_null()).unwrap_or(d));
    }
    if v["id"].as_i64().unwrap_or(0) == 0 {
        return Err(fail("id is not set".into()));
    }
    for k in ["tag_name", "name", "created_at", "published_at", "html_url"] {
        if v[k].as_str().is_none_or(str::is_empty) {
            return Err(fail(format!("{k} is not set")));
        }
    }
    let v = Value::Object(v);
    *CACHE.lock().unwrap() = Some((Instant::now(), v.clone()));
    Ok(json_raw(&v))
}

/// `getServerLimits`. This server has no user limit (Go's unlicensed one is 200 and 250), so the limits are 0
/// (none); system admins see the active user count.
async fn server_limits(St(app): St<State>, Auth(s): Auth) -> ApiResult<Json<Value>> {
    let admin = s.roles.split_whitespace().any(|r| r == crate::perm::SYSTEM_ADMIN)
        && app.has_perm(&s, "sysconsole_read_user_management_users").await;
    let active = if admin {
        app.count(
            "SELECT COUNT(*) FROM Users LEFT JOIN Bots ON Users.Id = Bots.UserId \
             WHERE Users.DeleteAt = 0 AND (Users.RemoteId = '' OR Users.RemoteId IS NULL) AND Bots.UserId IS NULL",
            "",
        )
        .await?
    } else {
        0
    };
    Ok(Json(json!({"maxUsersLimit": 0, "maxUsersHardLimit": 0, "activeUserCount": active, "singleChannelGuestCount": 0,
                   "singleChannelGuestLimit": 0, "postHistoryLimit": 0, "lastAccessiblePostTime": 0})))
}

// ---------- logs ----------

/// `getLogs`: this server logs to its standard error, not to a file the API could page through.
async fn get_logs(St(app): St<State>, Auth(s): Auth) -> ApiResult<Response> {
    app.require_unrestricted(&s, "get_logs").await?;
    Ok(json_raw(&json!([""])))
}

/// `queryLogs`: as `getLogs`, no log file, no lines.
async fn query_logs(St(app): St<State>, Auth(s): Auth, body: Bytes) -> ApiResult<Response> {
    app.require_unrestricted(&s, "get_logs").await?;
    match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(_)) => Ok(json_raw(&json!({}))),
        _ => Err(AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "api.system.logs.invalidFilter", "")),
    }
}

/// `postLog`: a client's message in the server log, at error level or (debug) otherwise.
async fn post_log(St(app): St<State>, MaybeAuth(s): MaybeAuth, headers: HeaderMap, body: Bytes) -> ApiResult<Json<Value>> {
    let mut force_debug = false;
    if !app.cfg.bool("ServiceSettings.EnableDeveloper") {
        let Some(s) = &s else { return Err(AppError::forbidden()) };
        if !app.has_perm(s, crate::perm::MANAGE_SYSTEM).await {
            force_debug = true;
        }
    }
    let mut m: Map<String, Value> = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(m)) => m.into_iter().filter(|(_, v)| v.is_string()).collect(),
        _ => Map::new(),
    };
    let level = m.get("level").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let mut msg = m.get("message").and_then(Value::as_str).unwrap_or("").to_string();
    if msg.len() > 400 {
        let mut cut = 399;
        while !msg.is_char_boundary(cut) {
            cut -= 1;
        }
        msg.truncate(cut);
    }
    let msg = format!("Client Logs API Endpoint Message: {msg}");
    let (sid, uid) = s.as_ref().map_or((String::new(), String::new()), |s| (s.id.clone(), s.user_id.clone()));
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !force_debug && level == "error" {
        eprintln!("error {msg} type=client_message user_agent={ua:?} session_id={sid} user_id={uid}");
    } else if std::env::var("LOG_REQUESTS").is_ok_and(|v| v == "1") {
        eprintln!("debug {msg} type=client_message user_agent={ua:?} session_id={sid} user_id={uid}");
    }
    m.insert("message".into(), Value::String(msg));
    Ok(Json(Value::Object(m)))
}

// ---------- first admin visit, onboarding ----------

async fn first_admin_visit(St(app): St<State>, Auth(s): Auth) -> ApiResult<Json<Value>> {
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    let v = app.admin_system_value("FirstAdminVisitMarketplace").await?.unwrap_or_else(|| "false".into());
    Ok(Json(json!({"name": "FirstAdminVisitMarketplace", "value": v})))
}

async fn set_first_admin_visit(St(app): St<State>, MaybeAuth(s): MaybeAuth) -> ApiResult<Json<Value>> {
    let Some(s) = s else { return Err(AppError::forbidden_perm(crate::perm::MANAGE_SYSTEM)) };
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    app.save_system("FirstAdminVisitMarketplace", "true")
        .await
        .map_err(|_| AppError::internal("api.error_set_first_admin_visit_marketplace_status", ""))?;
    app.hub.publish(
        "first_admin_visit_marketplace_status_received",
        json!({"firstAdminVisitMarketplaceStatus": "true"}),
        Default::default(),
    );
    Ok(ok())
}

/// `completeOnboarding`: the organization's name kept, the first admin's setup done (there are no plugins to
/// install).
async fn complete_onboarding(St(app): St<State>, Auth(s): Auth, body: Bytes) -> ApiResult<Json<Value>> {
    if !app.has_perm(&s, crate::perm::MANAGE_SYSTEM).await {
        return Err(AppError::new(StatusCode::FORBIDDEN, "app.system.complete_onboarding_request.no_first_user", ""));
    }
    let req: Value = serde_json::from_slice(&body)
        .ok()
        .filter(|v: &Value| v.is_object() || v.is_null())
        .ok_or_else(|| AppError::bad_request("app.system.complete_onboarding_request.app_error", ""))?;
    let org = req.get("organization").and_then(Value::as_str).unwrap_or("");
    if org.is_empty() {
        return Err(AppError::bad_request("api.error_no_organization_name_provided_for_self_hosted_onboarding", ""));
    }
    let _ = app.save_system("OrganizationName", org).await;
    app.save_system("FirstAdminSetupComplete", "true")
        .await
        .map_err(|_| AppError::internal("api.error_set_first_admin_complete_setup", ""))?;
    Ok(ok())
}

// ---------- server busy ----------

/// `Busy`: until when non-critical services are off (Unix seconds); none when not busy.
static BUSY: Mutex<Option<i64>> = Mutex::new(None);

fn busy_until() -> Option<i64> {
    let mut b = BUSY.lock().unwrap();
    if b.is_some_and(|t| t <= chrono::Utc::now().timestamp()) {
        *b = None;
    }
    *b
}

/// `Busy.ToJSON`: the zero time when not busy, as Go prints it.
async fn get_busy(St(app): St<State>, Auth(s): Auth) -> ApiResult<Response> {
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    let v = match busy_until() {
        Some(t) => {
            let ts = chrono::DateTime::from_timestamp(t, 0).unwrap_or_default();
            json!({"busy": true, "expires": t, "expires_ts": ts.format("%a %b %-d %H:%M:%S +0000 UTC %Y").to_string()})
        }
        None => json!({"busy": false, "expires": -62135596800i64, "expires_ts": "Mon Jan 1 00:00:00 +0000 UTC 0001"}),
    };
    Ok(json_raw(&v))
}

async fn set_busy(St(app): St<State>, Auth(s): Auth, Query(q): Q) -> ApiResult<Json<Value>> {
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    const MAX: i64 = 86400;
    let secs = match query(&q, "seconds") {
        "" => 3600,
        v => v.parse::<i64>().unwrap_or(-1),
    };
    if secs <= 0 || secs > MAX {
        return Err(AppError::invalid_url_param(&format!("seconds must be 1 - {MAX}")));
    }
    *BUSY.lock().unwrap() = Some(chrono::Utc::now().timestamp() + secs);
    Ok(ok())
}

async fn clear_busy(St(app): St<State>, Auth(s): Auth) -> ApiResult<Json<Value>> {
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    *BUSY.lock().unwrap() = None;
    Ok(ok())
}

// ---------- shared channels, schema, timezones ----------

/// `canUserDirectMessage`: without shared channels, anyone the user can see.
async fn can_dm(St(app): St<State>, Auth(s): Auth, Path((user_id, other)): Path<(String, String)>) -> ApiResult<Response> {
    let user_id = s.resolve(&user_id);
    if !is_valid_id(&user_id) {
        return Err(AppError::invalid_url_param("user_id"));
    }
    if !is_valid_id(&other) {
        return Err(AppError::invalid_url_param("other_user_id"));
    }
    let viewer = Session { user_id: user_id.clone(), ..(*s).clone() };
    let can = app.can_see_user(&viewer, &other).await?;
    Ok(json_raw(&json!({"can_dm": can})))
}

/// `getAppliedSchemaMigrations`.
async fn schema_version(St(app): St<State>, Auth(s): Auth) -> ApiResult<Response> {
    if !app.session_permissions(&s).await.iter().any(|p| p.starts_with("sysconsole_read_")) {
        return Err(AppError::forbidden_perm("sysconsole_read_*"));
    }
    let r: Vec<(i64, String)> = sqlx::query_as("SELECT Version::bigint, Name FROM db_migrations ORDER BY Version DESC")
        .fetch_all(&app.db)
        .await?;
    Ok(json_raw(&Value::Array(r.into_iter().map(|(v, n)| json!({"version": v, "name": n})).collect())))
}

/// `getSupportedTimezones` (`data/timezones.json`, Go's `DefaultSupportedTimezones`).
async fn timezones(_: Auth) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], include_str!("../data/timezones.json").trim().replace(['\n', ' '], ""))
        .into_response()
}

// ---------- enterprise upgrade, license, restart ----------

/// `upgradeToEnterprise`: this server cannot be replaced by an enterprise binary.
async fn upgrade_to_enterprise(St(app): St<State>, Auth(s): Auth) -> ApiResult<Response> {
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    Err(AppError::new(StatusCode::FORBIDDEN, "api.upgrade_to_enterprise.system_not_supported.app_error", ""))
}

/// `upgradeToEnterpriseStatus`: no upgrade ever started.
async fn upgrade_status(St(app): St<State>, Auth(s): Auth) -> ApiResult<Response> {
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    Ok(json_raw(&json!({"error": null, "percentage": 0})))
}

/// `addLicense` / `previewLicense`: a license file is read from the form, and no license validates here (there
/// is no license key to check it against): Go's answer to an invalid license.
async fn add_license(St(app): St<State>, Auth(s): Auth, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    app.require_unrestricted(&s, "manage_license_information").await?;
    let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let Some(boundary) = ct.strip_prefix("multipart/form-data").and_then(|r| r.split("boundary=").nth(1)) else {
        return Err(AppError::bad_request("api.license.parse_license.parse_form.app_error", ""));
    };
    let boundary = boundary.trim_matches('"');
    let text = String::from_utf8_lossy(&body);
    if !text.contains(&format!("--{boundary}")) {
        return Err(AppError::bad_request("api.license.parse_license.parse_form.app_error", ""));
    }
    if !text.contains("name=\"license\"") {
        return Err(AppError::bad_request("api.license.add_license.no_file.app_error", ""));
    }
    Err(AppError::bad_request("api.license.add_license.invalid.app_error", ""))
}

/// `restart`: answered, then the server starts again in place (the same program, arguments and environment), as
/// Go's `Server.Restart` does within its process.
async fn restart(St(app): St<State>, Auth(s): Auth) -> ApiResult<Json<Value>> {
    app.require(&s, crate::perm::MANAGE_SYSTEM).await?;
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        use std::os::unix::process::CommandExt;
        let Ok(exe) = std::env::current_exe() else { return };
        let err = std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec();
        eprintln!("restart: {err}");
    });
    Ok(ok())
}

// ---------- usage ----------

/// `RoundOffToZeroesResolution`.
fn round_off(n: i64, min_resolution: u32) -> i64 {
    if (-9..=9).contains(&n) {
        return if min_resolution == 0 { n } else { 0 };
    }
    let zeroes = (n.unsigned_abs() as f64).log10() as u32;
    let tens = 10i64.pow(zeroes.min(min_resolution));
    n / tens * tens
}

async fn usage_posts(St(app): St<State>, _: Auth) -> ApiResult<Response> {
    let n = app
        .count("SELECT COUNT(*) FROM Posts p WHERE p.Type = '' AND p.UserId NOT IN (SELECT UserId FROM Bots) AND p.DeleteAt = 0", "")
        .await
        .map_err(|_| AppError::internal("app.post.analytics_posts_count.app_error", ""))?;
    Ok(json_raw(&json!({"count": round_off(n, 3)})))
}

async fn usage_storage(St(app): St<State>, _: Auth) -> ApiResult<Response> {
    let n = app
        .count("SELECT COALESCE((SELECT usage FROM file_stats LIMIT 1), 0)::bigint", "")
        .await
        .map_err(|_| AppError::internal("app.usage.get_storage_usage.app_error", ""))?;
    Ok(json_raw(&json!({"bytes": round_off(n, 8)})))
}

async fn usage_teams(St(app): St<State>, _: Auth) -> ApiResult<Response> {
    let active = app.count("SELECT COUNT(*) FROM Teams WHERE DeleteAt = 0", "").await?;
    let archived = app.count("SELECT COUNT(*) FROM Teams WHERE DeleteAt > 0 AND CloudLimitsArchived", "").await?;
    Ok(json_raw(&json!({"active": active, "cloud_archived": archived})))
}

// ---------- product notices ----------

/// `updateViewedProductNotices`: each notice seen once more, now.
async fn view_notices(St(app): St<State>, Auth(s): Auth, body: Bytes) -> ApiResult<Json<Value>> {
    let ids: Vec<String> =
        serde_json::from_slice(&body).map_err(|_| AppError::bad_request("api.payload.parse.error", ""))?;
    let fail = |_| AppError::bad_request("api.system.update_viewed_notices.failed", "");
    let now = chrono::Utc::now().timestamp();
    let mut tx = app.db.begin().await?;
    let seen: Vec<String> =
        sqlx::query_scalar("SELECT NoticeId FROM ProductNoticeViewState WHERE UserId = $1 AND NoticeId = ANY($2)")
            .bind(&s.user_id)
            .bind(&ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(fail)?;
    sqlx::query("UPDATE ProductNoticeViewState SET Viewed = Viewed + 1, Timestamp = $3 WHERE UserId = $1 AND NoticeId = ANY($2)")
        .bind(&s.user_id)
        .bind(&seen)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(fail)?;
    for id in ids.iter().filter(|i| !seen.contains(i)) {
        sqlx::query("INSERT INTO ProductNoticeViewState (UserId, NoticeId, Viewed, Timestamp) VALUES ($1, $2, 1, $3)")
            .bind(&s.user_id)
            .bind(id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(fail)?;
    }
    tx.commit().await?;
    Ok(ok())
}

/// `getProductNotices`: notices come from the notices URL, which this server does not fetch: none.
async fn notices(Auth(_): Auth, Path(team_id): Path<String>, Query(q): Q) -> ApiResult<Response> {
    if !is_valid_id(&team_id) {
        return Err(AppError::invalid_url_param("team_id"));
    }
    // NoticeClientTypeFromString
    if !matches!(query(&q, "client"), "all" | "web" | "desktop" | "mobile-ios" | "mobile-android") {
        return Err(AppError::invalid_param("client"));
    }
    Ok(json_raw(&json!([])))
}

// ---------- system console tests ----------

/// `getRedirectLocation`: where a URL redirects to, kept an hour.
async fn redirect_location(St(app): St<State>, _: Auth, Query(q): Q) -> ApiResult<Response> {
    let mut m = Map::new();
    m.insert("location".into(), Value::String(String::new()));
    if !app.cfg.bool("ServiceSettings.EnableLinkPreviews") {
        return Ok(json_raw(&Value::Object(m)));
    }
    let url = query(&q, "url").to_string();
    if url.is_empty() {
        return Err(AppError::invalid_param("url"));
    }
    static CACHE: Mutex<Option<HashMap<String, (Instant, String)>>> = Mutex::new(None);
    let hit = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&url).cloned();
    let location = match hit {
        Some((at, l)) if at.elapsed() < Duration::from_secs(3600) => l,
        _ => {
            let l = match app.no_redirect_client().head(&url).send().await {
                Ok(r) => r.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string(),
                Err(_) => String::new(),
            };
            let mut c = CACHE.lock().unwrap();
            let c = c.get_or_insert_with(HashMap::new);
            if c.len() > 10000 {
                c.clear();
            }
            c.insert(url, (Instant::now(), l.clone()));
            l
        }
    };
    m.insert("location".into(), Value::String(location));
    Ok(json_raw(&Value::Object(m)))
}

/// `testSiteURL`: the URL must answer its ping.
async fn test_site_url(St(app): St<State>, Auth(s): Auth, body: Bytes) -> ApiResult<Json<Value>> {
    app.require_unrestricted(&s, "test_site_url").await?;
    let props: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let site = props.get("site_url").and_then(Value::as_str).unwrap_or("");
    if site.is_empty() {
        return Err(AppError::invalid_param("site_url"));
    }
    let ok_ping = match reqwest::get(format!("{site}/api/v4/system/ping")).await {
        Ok(r) => r.status() == reqwest::StatusCode::OK,
        Err(_) => false,
    };
    if !ok_ping {
        return Err(AppError::bad_request("app.admin.test_site_url.failure", ""));
    }
    Ok(ok())
}

/// The config a test request sends, or the server's when it sends none (Go logs a bad body and goes on).
fn test_config(app: &App, body: &Bytes) -> Value {
    match serde_json::from_slice::<Value>(body) {
        Ok(v @ Value::Object(_)) => v,
        _ => app.cfg.value().clone(),
    }
}

/// `checkHasNilFields` over a section of a test request's config.
fn has_nil_fields(cfg: &Value, section: &str) -> bool {
    match (crate::config::defaults().get(section).and_then(Value::as_object), cfg.get(section).and_then(Value::as_object)) {
        (Some(schema), Some(m)) => schema.keys().any(|k| m.get(k).is_none_or(Value::is_null)),
        _ => true,
    }
}

/// `testEmail`: this server sends no email; the settings are checked as Go checks them, and the SMTP server must
/// accept a connection.
async fn test_email(St(app): St<State>, Auth(s): Auth, body: Bytes) -> ApiResult<Json<Value>> {
    let cfg = test_config(&app, &body);
    if has_nil_fields(&cfg, "EmailSettings") {
        return Err(AppError::bad_request("api.file.test_connection_email_settings_nil.app_error", ""));
    }
    app.require_unrestricted(&s, "test_email").await?;
    let e = &cfg["EmailSettings"];
    let server = e["SMTPServer"].as_str().unwrap_or("");
    if server.is_empty() {
        return Err(AppError::internal("api.admin.test_email.missing_server", ""));
    }
    let port = e["SMTPPort"].as_str().unwrap_or("");
    let addr = format!("{server}:{port}");
    match tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => Ok(ok()),
        Ok(Err(err)) => Err(AppError::internal("api.admin.test_email.failure", err.to_string())),
        Err(_) => Err(AppError::internal("api.admin.test_email.failure", "timeout")),
    }
}

/// `testFileStore`: the store the request's file settings describe must take a file and give it back.
async fn test_file_store(St(app): St<State>, Auth(s): Auth, body: Bytes) -> ApiResult<Json<Value>> {
    let mut cfg = test_config(&app, &body);
    app.require(&s, "test_s3").await?;
    if has_nil_fields(&cfg, "FileSettings") {
        return Err(AppError::bad_request("api.file.test_connection_settings_nil.app_error", ""));
    }
    crate::config::desanitize(app.cfg.value(), &mut cfg);
    let files = crate::files::FileStore::new(&crate::config::Config::from_raw(cfg, String::new()))
        .map_err(|e| AppError::internal("api.file.no_driver.app_error", e.to_string()))?;
    let res = async {
        files.write("testfile", Bytes::from_static(b"testingwrite")).await?;
        files.remove("testfile").await
    }
    .await;
    res.map_err(|e| AppError::internal("api.file.test_connection.app_error", e.to_string()))?;
    Ok(ok())
}

/// `appendAncillaryPermissionsPost` (`data/ancillary_permissions.json`, Go's `SysconsoleAncillaryPermissions`).
async fn ancillary_permissions(_: Auth, body: Bytes) -> ApiResult<Response> {
    static MAP: std::sync::OnceLock<HashMap<String, Vec<String>>> = std::sync::OnceLock::new();
    let map = MAP.get_or_init(|| serde_json::from_str(include_str!("../data/ancillary_permissions.json")).unwrap_or_default());
    let perms: Vec<String> = serde_json::from_slice(&body)
        .ok()
        .filter(|p: &Vec<String>| !p.is_empty())
        .ok_or_else(|| AppError::bad_request("api.payload.parse.error", ""))?;
    let mut out = perms.clone();
    for p in &perms {
        if let Some(a) = map.get(p) {
            out.extend(a.iter().cloned());
        }
    }
    Ok(json_raw(&json!(out)))
}

// ---------- jobs ----------

/// `IsValidJobType`.
const JOB_TYPES: &[&str] = &[
    "data_retention", "message_export", "cli_message_export", "elasticsearch_post_indexing",
    "elasticsearch_post_aggregation", "ldap_sync", "migrations", "plugins", "expiry_notify", "product_notices",
    "active_users", "import_process", "import_delete", "export_process", "export_delete", "cloud",
    "resend_invitation_email", "extract_content", "last_accessible_post", "last_accessible_file",
    "upgrade_notify_admin", "trial_notify_admin", "post_persistent_notifications", "install_plugin_notify_admin",
    "hosted_purchase_screening", "cleanup_desktop_tokens", "delete_empty_drafts_migration",
    "refresh_materialized_views", "delete_orphan_drafts_migration", "export_users_to_csv",
    "delete_dms_preferences_migration", "mobile_session_metadata", "access_control_sync", "access_control_team_sync",
    "push_proxy_auth", "recap", "scheduled_recap", "delete_expired_posts", "autotranslation_recovery",
    "cleanup_expired_access_tokens", "notify_expiring_access_tokens",
];

/// `AllJobTypes`: what a listing without a type covers.
const ALL_JOB_TYPES: &[&str] = &[
    "data_retention", "message_export", "elasticsearch_post_indexing", "elasticsearch_post_aggregation", "ldap_sync",
    "migrations", "plugins", "expiry_notify", "product_notices", "active_users", "import_process", "import_delete",
    "export_process", "export_delete", "cloud", "extract_content", "last_accessible_post", "last_accessible_file",
    "cleanup_desktop_tokens", "cleanup_expired_access_tokens", "notify_expiring_access_tokens",
    "refresh_materialized_views", "mobile_session_metadata", "scheduled_recap",
];

/// `SessionHasPermissionToReadJob`: the permission a job type needs, if any.
async fn read_job_permission(app: &App, s: &Session, job_type: &str) -> Option<(bool, &'static str)> {
    let p = match job_type {
        "data_retention" => "read_data_retention_job",
        "message_export" => "read_compliance_export_job",
        "elasticsearch_post_indexing" => "read_elasticsearch_post_indexing_job",
        "elasticsearch_post_aggregation" => "read_elasticsearch_post_aggregation_job",
        "ldap_sync" => "read_ldap_sync_job",
        "migrations" | "plugins" | "product_notices" | "expiry_notify" | "active_users" | "import_process"
        | "import_delete" | "export_process" | "export_delete" | "cloud" | "mobile_session_metadata"
        | "extract_content" | "post_persistent_notifications" | "delete_expired_posts"
        | "cleanup_expired_access_tokens" => "read_jobs",
        "access_control_sync" => crate::perm::MANAGE_SYSTEM,
        "access_control_team_sync" => {
            if app.has_perm(s, crate::perm::MANAGE_SYSTEM).await {
                return Some((true, crate::perm::MANAGE_SYSTEM));
            }
            "manage_team_access_rules"
        }
        _ => return None,
    };
    Some((app.has_perm(s, p).await, p))
}

const JOB_COLS: &str = "Id, Type, Priority, CreateAt, StartAt, LastActivityAt, Status, Progress, Data::text";

type JobRow = (String, String, i64, i64, i64, i64, String, i64, Option<String>);

fn job_json(r: JobRow) -> Value {
    let (id, t, priority, create_at, start_at, last, status, progress, data) = r;
    let data: Value = data.and_then(|d| serde_json::from_str(&d).ok()).unwrap_or(Value::Null);
    json!({"id": id, "type": t, "priority": priority, "create_at": create_at, "start_at": start_at,
           "last_activity_at": last, "status": status, "progress": progress, "data": data})
}

async fn job_page(app: &App, types: &[&str], status: &str, page: i64, per_page: i64) -> ApiResult<Response> {
    let types: Vec<String> = types.iter().map(|t| t.to_string()).collect();
    let sql = format!(
        "SELECT {JOB_COLS} FROM Jobs WHERE Type = ANY($1){} ORDER BY CreateAt DESC LIMIT $2 OFFSET $3",
        if status.is_empty() { "" } else { " AND Status = $4" }
    );
    let mut q = sqlx::query_as::<_, JobRow>(sqlx::AssertSqlSafe(sql)).bind(types).bind(per_page).bind(page * per_page);
    if !status.is_empty() {
        q = q.bind(status);
    }
    let r = q.fetch_all(&app.db).await?;
    Ok(json_raw(&Value::Array(r.into_iter().map(job_json).collect())))
}

/// `getJobs`.
async fn jobs(St(app): St<State>, Auth(s): Auth, Query(q): Q) -> ApiResult<Response> {
    let job_type = query(&q, "job_type");
    let mut types = vec![];
    if !job_type.is_empty() {
        let Some(&t) = JOB_TYPES.iter().find(|t| **t == job_type) else {
            return Err(AppError::invalid_url_param("job_type"));
        };
        match read_job_permission(&app, &s, t).await {
            None => return Err(AppError::bad_request("api.job.retrieve.nopermissions", "")),
            Some((false, p)) => return Err(AppError::forbidden_perm(p)),
            Some((true, _)) => types.push(t),
        }
    } else {
        for t in ALL_JOB_TYPES {
            if let Some((true, _)) = read_job_permission(&app, &s, t).await {
                types.push(*t);
            }
        }
    }
    if types.is_empty() {
        return Err(AppError::forbidden());
    }
    let status = query(&q, "status");
    if !status.is_empty()
        && !matches!(status, "pending" | "in_progress" | "success" | "error" | "cancel_requested" | "canceled" | "warning")
    {
        return Err(AppError::bad_request("api.job.status.invalid", ""));
    }
    let (page, per_page) = page(&q);
    job_page(&app, &types, status, page, per_page).await
}

/// `getJobsByType`.
async fn jobs_by_type(St(app): St<State>, Auth(s): Auth, Path(job_type): Path<String>, Query(q): Q) -> ApiResult<Response> {
    if job_type.is_empty() || job_type.len() > 32 {
        return Err(AppError::invalid_url_param("job_type"));
    }
    match read_job_permission(&app, &s, &job_type).await {
        None => Err(AppError::bad_request("api.job.retrieve.nopermissions", "")),
        Some((false, p)) => Err(AppError::forbidden_perm(p)),
        Some((true, _)) => {
            let (page, per_page) = page(&q);
            job_page(&app, &[job_type.as_str()], "", page, per_page).await
        }
    }
}

/// `getJob`.
async fn job(St(app): St<State>, Auth(s): Auth, Path(job_id): Path<String>) -> ApiResult<Response> {
    if !is_valid_id(&job_id) {
        return Err(AppError::invalid_url_param("job_id"));
    }
    let r: Option<JobRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {JOB_COLS} FROM Jobs WHERE Id = $1")))
        .bind(&job_id)
        .fetch_optional(&app.db)
        .await?;
    let r = r.ok_or_else(|| AppError::not_found("app.job.get.app_error", ""))?;
    match read_job_permission(&app, &s, &r.1).await {
        None => Err(AppError::bad_request("api.job.retrieve.nopermissions", "")),
        Some((false, p)) => Err(AppError::forbidden_perm(p)),
        Some((true, _)) => Ok(json_raw(&job_json(r))),
    }
}

// ---------- user reports ----------

/// `fillUserReportOptions` + `applyUserReportFilter`: the condition on `Users u` and its binds.
fn user_report_filter(q: &HashMap<String, String>) -> ApiResult<(String, Vec<String>)> {
    let team = query(q, "team_filter");
    if !(team.is_empty() || is_valid_id(team)) {
        return Err(AppError::bad_request("api.getUsersForReporting.invalid_team_filter", ""));
    }
    let (hide_active, hide_inactive) = (query(q, "hide_active") == "true", query(q, "hide_inactive") == "true");
    if hide_active && hide_inactive {
        return Err(AppError::bad_request("api.getUsersForReporting.invalid_active_filter", ""));
    }
    let mut cond = vec!["u.Id NOT IN (SELECT UserId FROM Bots)".to_string()];
    let mut args: Vec<String> = vec![];
    let bind = |v: String, args: &mut Vec<String>| {
        args.push(v);
        format!("${}", args.len())
    };
    let channels = "(SELECT COUNT(*) FROM ChannelMembers cm INNER JOIN Channels c ON c.Id = cm.ChannelId AND c.DeleteAt = 0 \
                    AND c.Type IN ('O','P') WHERE cm.UserId = u.Id)";
    let role = match query(q, "guest_filter") {
        "all" => "system_guest",
        "single_channel" => {
            cond.push(format!("{channels} = 1"));
            "system_guest"
        }
        "multiple_channel" => {
            cond.push(format!("{channels} > 1"));
            "system_guest"
        }
        _ => query(q, "role_filter"),
    };
    let escape = |s: &str| s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    if !role.is_empty() {
        let p = bind(format!("%{}%", escape(role)), &mut args);
        cond.push(format!("u.Roles LIKE LOWER({p})"));
    }
    if query(q, "has_no_team") == "true" {
        cond.push("u.Id NOT IN (SELECT UserId FROM TeamMembers WHERE DeleteAt = 0)".into());
    } else if !team.is_empty() {
        let p = bind(team.to_string(), &mut args);
        cond.push(format!("u.Id IN (SELECT UserId FROM TeamMembers WHERE DeleteAt = 0 AND TeamId = {p})"));
    }
    if hide_active {
        cond.push("u.DeleteAt > 0".into());
    }
    if hide_inactive {
        cond.push("u.DeleteAt = 0".into());
    }
    for term in query(q, "search_term").split_whitespace() {
        let p = bind(format!("{}%", escape(term.trim_start_matches('@')).to_lowercase()), &mut args);
        cond.push(format!(
            "(LOWER(u.Username) LIKE {p} OR LOWER(u.FirstName) LIKE {p} OR LOWER(u.LastName) LIKE {p} \
             OR LOWER(u.Nickname) LIKE {p} OR LOWER(u.Email) LIKE {p})"
        ));
    }
    Ok((cond.join(" AND "), args))
}

/// `getUserCountForReporting`.
async fn user_report_count(St(app): St<State>, Auth(s): Auth, Query(q): Q) -> ApiResult<Response> {
    app.require(&s, "sysconsole_read_user_management_users").await?;
    let (cond, args) = user_report_filter(&q)?;
    let mut qy = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!("SELECT COUNT(u.Id) FROM Users u WHERE {cond}")));
    for a in args {
        qy = qy.bind(a);
    }
    Ok(json_raw(&json!(qy.fetch_one(&app.db).await?)))
}

/// What sorts a report. Go puts the requested column into its query as it comes; only these are taken here.
const REPORT_SORT: &[(&str, &str)] = &[
    ("username", "u.Username"),
    ("email", "u.Email"),
    ("createat", "u.CreateAt"),
    ("firstname", "u.FirstName"),
    ("lastname", "u.LastName"),
    ("nickname", "u.Nickname"),
    ("laststatusat", "LastStatusAt"),
    ("lastpostdate", "LastPostDate"),
    ("daysactive", "DaysActive"),
    ("totalposts", "TotalPosts"),
];

/// `getUsersForReporting`: a page of people (not bots) with their activity over all time.
async fn user_report(St(app): St<State>, Auth(s): Auth, Query(q): Q) -> ApiResult<Response> {
    app.require(&s, "sysconsole_read_user_management_users").await?;
    let col = match query(&q, "sort_column") {
        "" => "Username",
        c => c,
    };
    let col = REPORT_SORT.iter().find(|(k, _)| k.eq_ignore_ascii_case(col)).map_or("u.Username", |(_, v)| v);
    let prev = query(&q, "direction") == "prev";
    let desc = query(&q, "sort_direction") == "desc";
    let page_size = query(&q, "page_size").parse::<i64>().unwrap_or(50);
    let (cond, mut args) = user_report_filter(&q)?;
    if page_size <= 0 || page_size > 100 {
        return Err(AppError::bad_request("api.getUsersForReporting.invalid_page_size", ""));
    }
    let mut having = String::new();
    let mut dir = if desc { "DESC" } else { "ASC" };
    let (from_value, from_id) = (query(&q, "from_column_value"), query(&q, "from_id"));
    if !from_id.is_empty() && !from_value.is_empty() {
        args.push(from_value.to_string());
        let v = args.len();
        args.push(from_id.to_string());
        let i = args.len();
        let typed = if col.starts_with("u.") && col != "u.CreateAt" { format!("${v}") } else { format!("${v}::bigint") };
        let (cmp, d) = if prev != desc { ("<", "DESC") } else { (">", "ASC") };
        dir = d;
        having = format!("HAVING ({col} {cmp} {typed} OR ({col} = {typed} AND u.Id {cmp} ${i}))");
    }
    let sql = format!(
        "SELECT u.Id, MAX(s.LastActivityAt) AS LastStatusAt, MAX(ps.LastPostDate) AS LastPostDate, \
         COUNT(ps.Day) AS DaysActive, SUM(ps.NumPosts)::bigint AS TotalPosts, \
         (SELECT COUNT(*) FROM ChannelMembers cm INNER JOIN Channels c ON c.Id = cm.ChannelId AND c.DeleteAt = 0 \
          AND c.Type IN ('O','P') WHERE cm.UserId = u.Id) AS ChannelCount, \
         COALESCE((SELECT string_agg(t.DisplayName, ', ' ORDER BY t.DisplayName) FROM TeamMembers tm \
          INNER JOIN Teams t ON t.Id = tm.TeamId AND t.DeleteAt = 0 WHERE tm.UserId = u.Id AND tm.DeleteAt = 0), '') AS Teams \
         FROM Users u LEFT JOIN Status s ON s.UserId = u.Id LEFT JOIN PostStats ps ON ps.UserId = u.Id \
         WHERE {cond} GROUP BY u.Id {having} ORDER BY {col} {dir}, u.Id {dir} LIMIT {page_size}"
    );
    type Row = (String, Option<i64>, Option<i64>, i64, Option<i64>, i64, String);
    let mut qy = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(sql));
    for a in args {
        qy = qy.bind(a);
    }
    let mut rows = qy.fetch_all(&app.db).await?;
    if prev {
        rows.reverse();
    }
    let ids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let users = crate::users::query_users(&app, "", "u.Id = ANY($1)", "", vec![crate::users::Arg::V(ids)]).await?;
    // ClearNonProfileFields(true)
    const KEEP: &[&str] = &[
        "id", "create_at", "update_at", "delete_at", "username", "auth_service", "email", "nickname", "first_name",
        "last_name", "position", "roles", "notify_props", "locale", "timezone", "remote_id", "disable_welcome_email",
        "last_login",
    ];
    let out: Vec<Value> = rows
        .into_iter()
        .filter_map(|(id, status_at, post_date, days, posts, channels, teams)| {
            let u = users.iter().find(|u| u.id == id)?;
            let mut v = serde_json::to_value(u).ok()?;
            let m = v.as_object_mut()?;
            m.retain(|k, _| KEEP.contains(&k.as_str()));
            m.entry("remote_id").or_insert(json!(""));
            if let Some(t) = status_at {
                m.insert("last_status_at".into(), json!(t));
            }
            if let Some(t) = post_date {
                m.insert("last_post_date".into(), json!(t));
            }
            m.insert("days_active".into(), json!(days));
            if let Some(n) = posts {
                m.insert("total_posts".into(), json!(n));
            }
            m.insert("channel_count".into(), json!(channels));
            if !teams.is_empty() {
                m.insert("teams".into(), json!(teams));
            }
            Some(v)
        })
        .collect();
    Ok(json_raw(&Value::Array(out)))
}

// ---------- leftovers ----------

/// `removeUserRecentCustomStatus` (the `DELETE` form of `.../recent/delete`): Go checks the user id, whether
/// custom statuses are on, then the body.
async fn remove_recent_custom_status(
    st: St<State>,
    auth: Auth,
    Path(user_id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    if user_id != "me" && !is_valid_id(&user_id) {
        return Err(AppError::invalid_url_param("user_id"));
    }
    if !st.0.cfg.bool("TeamSettings.EnableCustomUserStatuses") {
        return Err(AppError::new(StatusCode::NOT_IMPLEMENTED, "api.custom_status.disabled", ""));
    }
    let cs: crate::user_edit::CustomStatus = serde_json::from_slice(&body)
        .map_err(|e| AppError::invalid_param("recent_custom_status").detail(e.to_string()))?;
    crate::user_edit::remove_recent_custom_status(st, auth, Path(user_id), crate::app::Body(cs)).await
}

/// `/manualtest`: Go's manual testing page, whose template a server build does not ship.
async fn manual_test() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "open root.html: no such file or directory\n",
    )
        .into_response()
}

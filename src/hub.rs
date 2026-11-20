//! Websocket connections and event broadcast, as Mattermost's platform hub does it on one node: an event is
//! serialised once, each connection adds its own `seq`; who gets it follows `WebConn.shouldSendEvent`.

// Based on Mattermost server/channels/app/platform/web_hub.go, web_conn.go and channels/wsapi, Copyright (c)
// 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, State as St};
use axum::http::request::Parts;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::app::{State, new_id, now_ms, token_from};
use crate::model::Session;

#[derive(Default)]
pub struct Hub {
    conns: RwLock<Vec<Arc<Conn>>>,
    next: AtomicU64,
    /// Events whose recipients depend on channel membership go through the app; see [`Hub::publish`].
    app: std::sync::OnceLock<std::sync::Weak<crate::app::App>>,
}

pub struct Conn {
    id: u64,
    pub user_id: String,
    pub connection_id: String,
    token: String,
    admin: bool,
    teams: RwLock<HashSet<String>>,
    tx: mpsc::UnboundedSender<Out>,
}

enum Out {
    /// An event without its `seq`, which the writer appends.
    Event(Arc<str>),
    /// A reply to the client's action: not numbered.
    Reply(String),
    Close,
}

/// `model.WebsocketBroadcast`: the part clients see, and how recipients are chosen.
#[derive(Serialize, Default, Clone)]
pub struct Broadcast {
    pub omit_users: Option<BTreeMap<String, bool>>,
    pub user_id: String,
    pub channel_id: String,
    pub team_id: String,
    pub connection_id: String,
    pub omit_connection_id: String,
    /// `ContainsSanitizedData`: non-admins get this data instead.
    #[serde(skip)]
    pub sanitized: Option<Value>,
    /// `ContainsSensitiveData`: admins only.
    #[serde(skip)]
    pub admin_only: bool,
    /// `RequiredPermissions`: only sessions with every one of these system permissions (it takes precedence
    /// over `admin_only`).
    #[serde(skip)]
    pub required_permissions: Vec<&'static str>,
    /// Broadcast hooks (`add_mentions`, `add_followers`): fields added to the data for one recipient.
    #[serde(skip)]
    pub per_user: Option<PerUser>,
}

/// Fields to add to an event's data for the user of a connection, if any.
pub type PerUser = Arc<dyn Fn(&str) -> Option<serde_json::Map<String, Value>> + Send + Sync>;

/// The event as JSON without its closing `"seq":N}`: `{"event":..,"data":..,"broadcast":..,"seq":`.
fn prefix(event: &str, data: &Value, b: &Broadcast) -> Arc<str> {
    // Go's field order (webSocketEventJSON); serde_json would sort the keys.
    Arc::from(format!(
        r#"{{"event":{},"data":{},"broadcast":{},"seq":"#,
        Value::from(event),
        data,
        serde_json::to_string(b).unwrap()
    ))
}

impl Hub {
    pub fn set_app(&self, app: &State) {
        let _ = self.app.set(Arc::downgrade(app));
    }

    /// The server state, for work that outlives a request.
    pub fn app(&self) -> Option<State> {
        self.app.get().and_then(std::sync::Weak::upgrade)
    }

    fn add(&self, c: Arc<Conn>) {
        self.conns.write().unwrap().push(c);
    }
    /// Removes a connection; true when it was the user's last one.
    fn remove(&self, id: u64) -> bool {
        let mut c = self.conns.write().unwrap();
        let Some(i) = c.iter().position(|x| x.id == id) else { return false };
        let gone = c.swap_remove(i);
        !c.iter().any(|x| x.user_id == gone.user_id)
    }

    pub fn is_connected(&self, user_id: &str) -> bool {
        self.conns.read().unwrap().iter().any(|c| c.user_id == user_id)
    }

    pub fn connections(&self) -> usize {
        self.conns.read().unwrap().len()
    }

    /// Closes the connections of a revoked session.
    pub fn close_session(&self, token: &str) {
        for c in self.conns.read().unwrap().iter().filter(|c| c.token == token) {
            let _ = c.tx.send(Out::Close);
        }
    }

    /// The user joined or left a team (`InvalidateCacheForWebConn`).
    pub fn set_user_teams(&self, user_id: &str, teams: &[String]) {
        for c in self.conns.read().unwrap().iter().filter(|c| c.user_id == user_id) {
            *c.teams.write().unwrap() = teams.iter().cloned().collect();
        }
    }

    /// `Publish`: to every connection the broadcast selects. Channel events go to members of the channel, as
    /// `GetAllChannelMembersForUser` (cached) says.
    pub fn publish(&self, event: &str, data: Value, b: Broadcast) {
        let targets: Vec<Arc<Conn>> = {
            let conns = self.conns.read().unwrap();
            conns
                .iter()
                .filter(|c| {
                    if !b.connection_id.is_empty() {
                        return c.connection_id == b.connection_id;
                    }
                    if !b.omit_connection_id.is_empty() && c.connection_id == b.omit_connection_id {
                        return false;
                    }
                    if !b.user_id.is_empty() {
                        return c.user_id == b.user_id;
                    }
                    if b.omit_users.as_ref().is_some_and(|o| o.contains_key(&c.user_id)) {
                        return false;
                    }
                    if b.admin_only && b.required_permissions.is_empty() && !c.admin {
                        return false;
                    }
                    if !b.team_id.is_empty() && b.channel_id.is_empty() {
                        return c.teams.read().unwrap().contains(&b.team_id);
                    }
                    true
                })
                .cloned()
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        let full = prefix(event, &data, &b);
        let sanitized = b.sanitized.as_ref().map(|d| prefix(event, d, &b));
        let event = event.to_string();
        let channel_id = b.channel_id.clone();
        let required = b.required_permissions.clone();
        // The event for one connection: admin or sanitized data, and the hooks' fields.
        let render = move |c: &Conn| -> Arc<str> {
            let base = if c.admin { &full } else { sanitized.as_ref().unwrap_or(&full) };
            let Some(extra) = b.per_user.as_ref().and_then(|f| f(&c.user_id)) else { return base.clone() };
            let mut d = if c.admin { data.clone() } else { b.sanitized.clone().unwrap_or_else(|| data.clone()) };
            if let Value::Object(m) = &mut d {
                m.extend(extra);
            }
            prefix(&event, &d, &b)
        };
        if !required.is_empty() {
            // The permissions of each connection's session, off the caller's path.
            let Some(app) = self.app.get().and_then(|w| w.upgrade()) else { return };
            tokio::spawn(async move {
                for c in targets {
                    let Ok(s) = app.session(&c.token).await else { continue };
                    let mut ok = true;
                    for p in &required {
                        ok = ok && app.has_perm(&s, p).await;
                    }
                    if ok {
                        let _ = c.tx.send(Out::Event(render(&c)));
                    }
                }
            });
            return;
        }
        if channel_id.is_empty() {
            for c in targets {
                let _ = c.tx.send(Out::Event(render(&c)));
            }
            return;
        }
        // Membership needs the (cached) channel members of each user: done off the caller's path.
        let Some(app) = self.app.get().and_then(|w| w.upgrade()) else { return };
        tokio::spawn(async move {
            for c in targets {
                let member = match app.member_roles(&c.user_id, false).await {
                    Ok(m) => m.contains_key(&channel_id),
                    Err(_) => false,
                };
                if member {
                    let _ = c.tx.send(Out::Event(render(&c)));
                }
            }
        });
    }
}

/// `connectWebSocket` behind `APIHandlerTrustRequester`: the session of the request's token (a token Go cannot
/// find is no session: the client may still authenticate on the socket), then gorilla's `Upgrader.Upgrade` with
/// `App.OriginChecker`.
pub async fn connect(St(app): St<State>, req: axum::extract::Request) -> Response {
    use axum::response::IntoResponse;
    let (mut parts, _) = req.into_parts();
    let mut token = None;
    if let Some((t, loc)) = token_from(&parts) {
        match app.session(&t).await {
            // web/handlers.go: a token in the query string is for OAuth apps only.
            Ok(s) if loc == crate::app::TokenLocation::Query && !s.is_oauth => {
                return crate::app::AppError::new(
                    axum::http::StatusCode::UNAUTHORIZED,
                    "api.context.token_provided.app_error",
                    "Session is not OAuth but token was provided in the query string",
                )
                .into_response();
            }
            Ok(_) => token = Some(t),
            Err(e) if e.status_code == 500 => return e.into_response(),
            Err(_) => {}
        }
    }
    let origin = parts.headers.get("origin").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    if let Err(status) = handshake_check(&app, &parts) {
        return upgrade_error(status, origin.as_deref().unwrap_or(""));
    }
    match WebSocketUpgrade::from_request_parts(&mut parts, &app).await {
        Ok(ws) => ws.max_message_size(8 * 1024).on_upgrade(move |socket| serve(app, socket, token)),
        Err(_) => upgrade_error(axum::http::StatusCode::BAD_REQUEST, origin.as_deref().unwrap_or("")),
    }
}

/// gorilla's `tokenListContainsValue`: one of the header's comma-separated tokens, any case.
fn header_has_token(parts: &Parts, name: &str, value: &str) -> bool {
    parts.headers.get_all(name).iter().any(|v| {
        String::from_utf8_lossy(v.as_bytes()).split(',').any(|t| t.trim_matches([' ', '\t']).eq_ignore_ascii_case(value))
    })
}

/// `Upgrader.Upgrade`'s checks, in its order: the status it refuses with.
fn handshake_check(app: &crate::app::App, parts: &Parts) -> Result<(), axum::http::StatusCode> {
    use axum::http::StatusCode;
    if !header_has_token(parts, "connection", "upgrade") || !header_has_token(parts, "upgrade", "websocket") {
        return Err(StatusCode::BAD_REQUEST);
    }
    if parts.method != axum::http::Method::GET {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    if !header_has_token(parts, "sec-websocket-version", "13") {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !origin_allowed(app, parts) {
        return Err(StatusCode::FORBIDDEN);
    }
    let key = parts.headers.get("sec-websocket-key").map(|v| v.as_bytes()).unwrap_or_default();
    use base64::Engine;
    if key.is_empty() || base64::engine::general_purpose::STANDARD.decode(key).map(|k| k.len()) != Ok(16) {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

/// `App.OriginChecker`: with `AllowCorsFrom`, those origins and the site URL (or any, for `*`); without it, the
/// origin must be the site URL's scheme and host (the request's host when there is no site URL). No `Origin` is
/// fine (not a browser), `null` is not.
fn origin_allowed(app: &crate::app::App, parts: &Parts) -> bool {
    let site = app.cfg.str("ServiceSettings.SiteURL");
    let cors = app.cfg.str("ServiceSettings.AllowCorsFrom");
    let first = parts.headers.get("origin").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    if !cors.is_empty() {
        // utils.CheckOrigin
        let origin = first.unwrap_or_default();
        if origin.is_empty() || cors == "*" {
            return true;
        }
        let mut allowed = cors.to_string();
        if let Ok(mut u) = crate::gourl::Url::parse(site) {
            u.raw_path.clear();
            allowed.push(' ');
            allowed.push_str(&u.to_go_string());
        }
        return allowed.split(' ').any(|a| a == origin);
    }
    let Some(origin) = first else { return true };
    if origin == "null" {
        return false;
    }
    let Ok(u) = crate::gourl::Url::parse(&origin) else { return false };
    if site.is_empty() {
        let host = parts.headers.get("host").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
        let host = host.or_else(|| parts.uri.authority().map(|a| a.to_string())).unwrap_or_default();
        return u.host.eq_ignore_ascii_case(&host);
    }
    let Ok(s) = crate::gourl::Url::parse(site) else { return false };
    u.host.eq_ignore_ascii_case(&s.host) && u.scheme.eq_ignore_ascii_case(&s.scheme)
}

/// A refused upgrade as Go answers it: gorilla's `http.Error` (its status, `Sec-Websocket-Version: 13`, the
/// status text), then the handler's `api.web_socket.connect.upgrade.app_error` (400 in the JSON) written after it.
fn upgrade_error(status: axum::http::StatusCode, origin: &str) -> Response {
    use axum::http::{HeaderValue, header};
    let e = crate::app::AppError::new(axum::http::StatusCode::BAD_REQUEST, "api.web_socket.connect.upgrade.app_error", "")
        .with_param("BlockedOrigin", origin);
    let rid = e.request_id.clone();
    let json = serde_json::to_string(&e).unwrap_or_default();
    let body = format!("{}\n{json}", status.canonical_reason().unwrap_or(""));
    let mut r = Response::new(axum::body::Body::from(body));
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    h.insert("sec-websocket-version", HeaderValue::from_static("13"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    if let Ok(v) = HeaderValue::from_str(&rid) {
        h.insert("x-request-id", v);
    }
    r
}

async fn serve(app: State, socket: WebSocket, mut token: Option<String>) {
    let (mut sink, mut stream) = socket.split();
    if token.is_none() {
        // {"seq":1,"action":"authentication_challenge","data":{"token":"..."}}
        if let Some(Ok(Message::Text(t))) = stream.next().await {
            let v: Value = serde_json::from_str(&t).unwrap_or_default();
            if v["action"] == "authentication_challenge" {
                token = v["data"]["token"].as_str().map(str::to_string);
                let reply = json!({"status": "OK", "seq_reply": v["seq"]});
                let _ = sink.send(Message::Text(reply.to_string().into())).await;
            }
        }
    }
    let Some(session) = (match &token {
        Some(t) => app.session(t).await.ok(),
        None => None,
    }) else {
        let _ = sink.close().await;
        return;
    };
    let user_id = session.user_id.clone();
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let conn = Arc::new(Conn {
        id: app.hub.next.fetch_add(1, Ordering::Relaxed),
        user_id: user_id.clone(),
        connection_id: new_id(),
        token: session.token.clone(),
        admin: session.is_system_admin(),
        teams: RwLock::new(session.team_members.iter().map(|t| t.team_id.clone()).collect()),
        tx,
    });
    app.hub.add(conn.clone());

    let writer = tokio::spawn(async move {
        let mut seq: i64 = 0;
        let mut ping = tokio::time::interval(Duration::from_secs(60));
        ping.tick().await;
        loop {
            tokio::select! {
                m = rx.recv() => {
                    let p = match m {
                        Some(Out::Event(p)) => p,
                        Some(Out::Reply(r)) => {
                            if sink.send(Message::Text(r.into())).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        _ => break,
                    };
                    let mut s = String::with_capacity(p.len() + 24);
                    s.push_str(&p);
                    s.push_str(&seq.to_string());
                    s.push('}');
                    seq += 1;
                    if sink.send(Message::Text(Utf8Bytes::from(s))).await.is_err() {
                        break;
                    }
                }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
    });

    let hello = json!({
        "connection_id": conn.connection_id,
        "server_version": format!("{}.{}.{}.false", crate::SERVER_VERSION_FULL, crate::SERVER_VERSION, "0"),
    });
    app.hub.publish("hello", hello, Broadcast { connection_id: conn.connection_id.clone(), ..Default::default() });
    app.set_status_online(&user_id, false).await;

    while let Some(Ok(m)) = stream.next().await {
        match m {
            Message::Text(t) => action(&app, &session, &conn, &t).await,
            Message::Close(_) => break,
            _ => {}
        }
        if writer.is_finished() {
            break;
        }
    }
    writer.abort();
    if app.hub.remove(conn.id) {
        app.queue_status_offline(&user_id, false).await;
    }
}

/// `model.WebSocketResponse`, in Go's field order.
#[derive(Serialize)]
struct WsReply {
    status: &'static str,
    #[serde(skip_serializing_if = "is_zero")]
    seq_reply: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<WsReplyError>,
}

/// The `AppError` of a failed socket request, its detail wiped (`WipeDetailed`).
#[derive(Serialize)]
struct WsReplyError {
    id: &'static str,
    message: String,
    detailed_error: &'static str,
    status_code: u16,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Why a socket request failed: `NewInvalidWebSocketParamError`, `NewServerBusyWebSocketError`.
enum WsError {
    Param(&'static str),
    Busy,
}

/// A client's request on the socket (`wsapi`), answered with `{"status","seq_reply"}`.
async fn action(app: &State, s: &Arc<Session>, conn: &Conn, text: &str) {
    let Ok(req) = serde_json::from_str::<Value>(text) else { return };
    let seq = req["seq"].clone();
    let data = &req["data"];
    let reply = match req["action"].as_str().unwrap_or("") {
        "user_typing" => 'typing: {
            app.extend_session_if_needed(s).await;
            if crate::admin::is_busy() {
                // A non-critical service, off while the server is busy.
                break 'typing Err(WsError::Busy);
            }
            let channel_id = data["channel_id"].as_str().unwrap_or("");
            if crate::app::is_valid_id(channel_id)
                && app.channel_perm(s, channel_id, crate::perm::CREATE_POST).await.is_ok_and(|p| p.0)
            {
                publish_typing(app, &s.user_id, channel_id, data["parent_id"].as_str().unwrap_or(""));
                Ok(None)
            } else {
                Err(WsError::Param("channel_id"))
            }
        }
        "user_update_active_status" => match data["user_is_active"].as_bool() {
            Some(active) => {
                let manual = data["manual"].as_bool().unwrap_or(false);
                if active {
                    app.set_status_online(&s.user_id, manual).await;
                } else {
                    app.set_status_away_if_needed(&s.user_id, manual).await;
                }
                app.touch_session(s).await;
                Ok(None)
            }
            None => Err(WsError::Param("user_is_active")),
        },
        "get_statuses" => Ok(Some(json!({}))),
        "get_statuses_by_ids" => {
            let ids: Vec<String> = data["user_ids"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            if ids.is_empty() {
                Err(WsError::Param("user_ids"))
            } else {
                let st = app.statuses(&ids).await.unwrap_or_default();
                Ok(Some(Value::Object(st.into_iter().map(|s| (s.user_id, Value::String(s.status))).collect())))
            }
        }
        "ping" => Ok(Some(json!({"text": "pong", "version": crate::SERVER_VERSION_FULL, "server_time": now_ms()}))),
        _ => Err(WsError::Param("action")),
    };
    let seq_reply = seq.as_i64().unwrap_or(0);
    let msg = match reply {
        Ok(data) => WsReply { status: "OK", seq_reply, data, error: None },
        Err(e) => {
            let (id, status_code, args): (&str, u16, &[(&str, &str)]) = match &e {
                WsError::Param(name) => ("api.websocket_handler.invalid_param.app_error", 400, &[("Name", name)]),
                WsError::Busy => ("api.websocket_handler.server_busy.app_error", 503, &[]),
            };
            let message = crate::i18n::tr(id, args).unwrap_or_else(|| id.to_string());
            let error = WsReplyError { id, message, detailed_error: "", status_code };
            WsReply { status: "FAIL", seq_reply, data: None, error: Some(error) }
        }
    };
    let msg = serde_json::to_string(&msg).unwrap_or_default();
    let _ = conn.tx.send(Out::Reply(msg));
}

/// `PublishUserTyping`.
pub fn publish_typing(app: &State, user_id: &str, channel_id: &str, parent_id: &str) {
    let mut omit = BTreeMap::new();
    omit.insert(user_id.to_string(), true);
    app.hub.publish(
        "typing",
        json!({"parent_id": parent_id, "user_id": user_id}),
        Broadcast { channel_id: channel_id.into(), omit_users: Some(omit), ..Default::default() },
    );
}

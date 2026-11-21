//! Websocket connections and event broadcast, as Mattermost's platform hub does it on one node: an event is
//! serialised once, each connection adds its own `seq`; who gets it follows `WebConn.shouldSendEvent`.

// Based on Mattermost server/channels/app/platform/web_hub.go, web_conn.go and channels/wsapi, Copyright (c)
// 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
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
    /// The session events are filtered with (`WebConn.session`): the connect's, then the one reloaded after an
    /// invalidation or its expiry. `admin`, `guest`, `expires_at` and `teams` are read from it, so that the hot
    /// path is a few atomic loads.
    session: RwLock<Arc<Session>>,
    admin: AtomicBool,
    guest: AtomicBool,
    /// `sessionExpiresAt` (0: never).
    expires_at: AtomicI64,
    /// `InvalidateCache` bumps `generation`; the session is loaded again while `loaded` differs.
    generation: AtomicU64,
    loaded: AtomicU64,
    /// Go's cleared session token: the session could not be loaded again (or was revoked), nothing more is sent.
    dead: AtomicBool,
    teams: RwLock<HashSet<String>>,
    tx: mpsc::Sender<Out>,
    /// Wakes the writer to close the socket: a full queue cannot take `Out::Close`.
    closing: Arc<tokio::sync::Notify>,
}

/// `sendQueueSize`: events a connection may have waiting. A client that lets it fill is closed, as Go's hub does
/// (`webhub.broadcast: cannot send, closing websocket`); it reconnects and loads what it missed. Without the bound
/// one stuck client would hold every event sent to it in memory.
const SEND_QUEUE: usize = 256;

impl Conn {
    fn new(id: u64, session: Arc<Session>, tx: mpsc::Sender<Out>) -> Conn {
        let c = Conn {
            id,
            user_id: session.user_id.clone(),
            connection_id: new_id(),
            token: session.token.clone(),
            session: RwLock::new(session.clone()),
            admin: AtomicBool::new(false),
            guest: AtomicBool::new(false),
            expires_at: AtomicI64::new(0),
            generation: AtomicU64::new(0),
            loaded: AtomicU64::new(0),
            dead: AtomicBool::new(false),
            teams: RwLock::new(HashSet::new()),
            tx,
            closing: Arc::new(tokio::sync::Notify::new()),
        };
        c.set_session(session);
        c
    }

    /// `SetSession` and `SetSessionExpiresAt`, with what the filters read from the session.
    fn set_session(&self, s: Arc<Session>) {
        self.admin.store(s.is_system_admin(), Ordering::Relaxed);
        self.guest.store(s.prop("is_guest") == "true", Ordering::Relaxed);
        self.expires_at.store(s.expires_at, Ordering::Relaxed);
        *self.teams.write().unwrap() = s.team_members.iter().map(|t| t.team_id.clone()).collect();
        *self.session.write().unwrap() = s;
    }

    fn session(&self) -> Arc<Session> {
        self.session.read().unwrap().clone()
    }

    /// Queues a frame for the writer; a full queue closes the connection (Go's `closeAndRemoveConn`).
    fn send(&self, out: Out) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(out)
            && !self.dead.swap(true, Ordering::Relaxed)
        {
            crate::logs::error(
                "webhub.broadcast: cannot send, closing websocket for user",
                &[("user_id", json!(self.user_id)), ("conn_id", json!(self.connection_id))],
            );
            self.closing.notify_one();
        }
    }

    /// Closes the socket; nothing more is sent to it meanwhile.
    fn close(&self) {
        self.dead.store(true, Ordering::Relaxed);
        self.closing.notify_one();
    }

    fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    /// Whether the session must be loaded again before an event goes out: invalidated (`InvalidateCache`) or past
    /// its expiry (`IsBasicAuthenticated`).
    fn is_stale(&self, now: i64) -> bool {
        let exp = self.expires_at.load(Ordering::Relaxed);
        self.generation.load(Ordering::Acquire) != self.loaded.load(Ordering::Acquire) || (exp > 0 && exp < now)
    }

    /// `IsBasicAuthenticated`: the session as it is now when it is stale. A session that is gone or invalid ends
    /// the deliveries as Go's cleared token does (the socket stays open; its requests fail with the session
    /// error). A server error closes the socket instead of silencing it for good (Go clears the token on any
    /// error): the client reconnects and loads what it missed.
    async fn authenticate(&self, app: &crate::app::App) -> bool {
        if self.is_dead() {
            return false;
        }
        if !self.is_stale(now_ms()) {
            return true;
        }
        let generation = self.generation.load(Ordering::Acquire);
        match app.session(&self.token).await {
            Ok(s) => {
                self.set_session(s);
                self.loaded.store(generation, Ordering::Release);
                true
            }
            Err(e) => {
                self.dead.store(true, Ordering::Relaxed);
                if e.status_code >= 500 {
                    self.closing.notify_one();
                }
                false
            }
        }
    }
}

enum Out {
    /// An event without its `seq`, which the writer appends.
    Event(Arc<str>),
    /// A reply to the client's action: not numbered.
    Reply(String),
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
    /// A hook that needs the app (`channelMentionsBroadcastHook`): the event's `post` for the user of a connection,
    /// when it is not the shared one.
    #[serde(skip)]
    pub post_hook: Option<PostHook>,
}

/// Fields to add to an event's data for the user of a connection, if any.
pub type PerUser = Arc<dyn Fn(&str) -> Option<serde_json::Map<String, Value>> + Send + Sync>;

/// The `post` of an event for one recipient (by user id), when it differs from the shared one.
pub type PostHook = Arc<dyn Fn(State, String) -> futures_util::future::BoxFuture<'static, Option<String>> + Send + Sync>;

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

/// One published event and what `ShouldSendEvent` checks for it, shared by the connections it goes to.
struct Delivery {
    event: String,
    data: Value,
    b: Broadcast,
    full: Arc<str>,
    sanitized: Option<Arc<str>>,
    /// The broadcast names its connection or user: no channel, team or guest check (Go returns before them).
    routed: bool,
    /// `ShouldSendEventToGuest`: the user a guest must be able to see (`user_updated`, `new_user`), when the
    /// event reaches that check (no channel, team, connection or user).
    guest_subject: Option<String>,
}

impl Delivery {
    /// The checks on the connection's session that need no I/O: sensitive data, the team.
    fn session_allows(&self, c: &Conn) -> bool {
        if self.b.admin_only && self.b.required_permissions.is_empty() && !c.admin.load(Ordering::Relaxed) {
            return false;
        }
        if !self.routed && self.b.channel_id.is_empty() && !self.b.team_id.is_empty() {
            return c.teams.read().unwrap().contains(&self.b.team_id);
        }
        true
    }

    /// Whether a connection whose session is loaded and allowed needs no async check.
    fn is_plain(&self, c: &Conn) -> bool {
        self.b.required_permissions.is_empty()
            && (self.routed || self.b.channel_id.is_empty())
            && self.b.post_hook.is_none()
            && !(self.guest_subject.is_some() && c.guest.load(Ordering::Relaxed))
    }

    /// `ShouldSendEvent` for a connection, with what needs the app: the session reloaded when stale, the required
    /// permissions, channel membership (`GetAllChannelMembersForUser`, cached), the guest's view of the user.
    async fn allows(&self, app: &crate::app::App, c: &Conn) -> bool {
        if !c.authenticate(app).await || !self.session_allows(c) {
            return false;
        }
        if !self.b.required_permissions.is_empty() {
            let s = c.session();
            for p in &self.b.required_permissions {
                if !app.has_perm(&s, p).await {
                    return false;
                }
            }
        }
        if !self.routed && !self.b.channel_id.is_empty() {
            // Archived channels count: Go checks the connection's own copy of the memberships, loaded before the
            // archive (an archive invalidates no user), so the "archived the channel" post still reaches the
            // members. Nothing else can be posted to an archived channel.
            match app.member_roles(&c.user_id, true).await {
                Ok(m) if m.contains_key(&self.b.channel_id) => {}
                _ => return false,
            }
        }
        if let Some(subject) = &self.guest_subject
            && c.guest.load(Ordering::Relaxed)
        {
            // An error means no delivery, as Go.
            return matches!(app.can_see_user(&c.session(), subject).await, Ok(true));
        }
        true
    }

    /// The event for one connection: admin or sanitized data, and the hooks' fields.
    fn render(&self, c: &Conn, post: Option<String>) -> Arc<str> {
        let admin = c.admin.load(Ordering::Relaxed);
        let base = if admin { &self.full } else { self.sanitized.as_ref().unwrap_or(&self.full) };
        let mut extra = self.b.per_user.as_ref().and_then(|f| f(&c.user_id));
        if let Some(p) = post {
            extra.get_or_insert_with(serde_json::Map::new).insert("post".into(), Value::String(p));
        }
        let Some(extra) = extra else { return base.clone() };
        let mut d = if admin { self.data.clone() } else { self.b.sanitized.clone().unwrap_or_else(|| self.data.clone()) };
        if let Value::Object(m) = &mut d {
            m.extend(extra);
        }
        prefix(&self.event, &d, &self.b)
    }

    /// The post hook's answer for the user of a connection.
    async fn post_for(&self, app: &State, c: &Conn) -> Option<String> {
        match &self.b.post_hook {
            Some(h) => h(app.clone(), c.user_id.clone()).await,
            None => None,
        }
    }
}

/// `ShouldSendEventToGuest`'s subject: the user of `user_updated` (`data.user.id`), the `user_id` of `new_user`.
/// An event without one goes to no guest (an empty id is a user nobody can see).
fn guest_subject(event: &str, data: &Value) -> Option<String> {
    let id = match event {
        "user_updated" => &data["user"]["id"],
        "new_user" => &data["user_id"],
        _ => return None,
    };
    Some(id.as_str().unwrap_or_default().to_string())
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

    /// Closes the connections of a revoked session; nothing more is sent to them meanwhile.
    pub fn close_session(&self, token: &str) {
        for c in self.conns.read().unwrap().iter().filter(|c| c.token == token) {
            c.close();
        }
    }

    /// `Hub.InvalidateUser` (`WebConn.InvalidateCache`): the user's connections load their session again before
    /// their next event (roles, guest, teams, a revoked or expired session).
    pub fn invalidate_user(&self, user_id: &str) {
        for c in self.conns.read().unwrap().iter().filter(|c| c.user_id == user_id) {
            c.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// The connections of one session load it again before their next event (a revoked OAuth token).
    pub fn invalidate_session(&self, token: &str) {
        for c in self.conns.read().unwrap().iter().filter(|c| c.token == token) {
            c.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// `Hub.InvalidateAllCache`: every connection loads its session again before its next event.
    pub fn invalidate_all(&self) {
        for c in self.conns.read().unwrap().iter() {
            c.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// The user joined or left a team (`InvalidateCacheForWebConn`).
    pub fn set_user_teams(&self, user_id: &str, teams: &[String]) {
        for c in self.conns.read().unwrap().iter().filter(|c| c.user_id == user_id) {
            *c.teams.write().unwrap() = teams.iter().cloned().collect();
        }
    }

    /// `Publish`: to every connection `ShouldSendEvent` selects. A connection whose session is loaded and that
    /// needs nothing more gets the event here; the others (a stale session, channel membership, required
    /// permissions, a guest's view, a post hook) get it off the caller's path.
    pub fn publish(&self, event: &str, data: Value, b: Broadcast) {
        let now = now_ms();
        let routed = !b.connection_id.is_empty() || !b.user_id.is_empty();
        let subject = if !routed && b.channel_id.is_empty() && b.team_id.is_empty() { guest_subject(event, &data) } else { None };
        let mut later: Vec<Arc<Conn>> = Vec::new();
        let mut now_conns: Vec<Arc<Conn>> = Vec::new();
        {
            let conns = self.conns.read().unwrap();
            for c in conns.iter() {
                // The broadcast's own routing, which needs no session.
                if !b.connection_id.is_empty() {
                    if c.connection_id != b.connection_id {
                        continue;
                    }
                } else if !b.omit_connection_id.is_empty() && c.connection_id == b.omit_connection_id {
                    continue;
                } else if !b.user_id.is_empty() {
                    if c.user_id != b.user_id {
                        continue;
                    }
                } else if b.omit_users.as_ref().is_some_and(|o| o.contains_key(&c.user_id)) {
                    continue;
                }
                if c.is_dead() {
                    continue;
                }
                if c.is_stale(now) {
                    later.push(c.clone());
                } else {
                    now_conns.push(c.clone());
                }
            }
        }
        if later.is_empty() && now_conns.is_empty() {
            return;
        }
        let dl = Delivery {
            event: event.to_string(),
            full: prefix(event, &data, &b),
            sanitized: b.sanitized.as_ref().map(|s| prefix(event, s, &b)),
            data,
            b,
            routed,
            guest_subject: subject,
        };
        for c in now_conns {
            if !dl.session_allows(&c) {
                continue;
            }
            if dl.is_plain(&c) {
                c.send(Out::Event(dl.render(&c, None)));
            } else {
                later.push(c);
            }
        }
        if later.is_empty() {
            return;
        }
        let Some(app) = self.app() else { return };
        tokio::spawn(async move {
            for c in later {
                if dl.allows(&app, &c).await {
                    let post = dl.post_for(&app, &c).await;
                    c.send(Out::Event(dl.render(&c, post)));
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
    let (tx, mut rx) = mpsc::channel::<Out>(SEND_QUEUE);
    let conn = Arc::new(Conn::new(app.hub.next.fetch_add(1, Ordering::Relaxed), session.clone(), tx));
    app.hub.add(conn.clone());

    let closing = conn.closing.clone();
    let mut writer = tokio::spawn(async move {
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
                        None => break,
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
                _ = closing.notified() => break,
            }
        }
        let _ = sink.close().await;
    });

    let hello = json!({
        "connection_id": conn.connection_id,
        "server_version": format!("{}.{}.{}.false", crate::SERVER_VERSION_FULL, crate::SERVER_VERSION, "0"),
    });
    app.hub.publish("hello", hello, Broadcast { connection_id: conn.connection_id.clone(), ..Default::default() });
    // `NewWebConn`: `UpdateLastActivityAtIfNeeded` once, with the session just loaded.
    app.touch_session(&session).await;
    app.set_status_online(&user_id, false).await;

    loop {
        // The writer ending (closed, full queue, broken socket) ends the connection without waiting for the
        // client's next frame.
        let m = tokio::select! {
            m = stream.next() => m,
            _ = &mut writer => break,
        };
        let Some(Ok(m)) = m else { break };
        match m {
            // `ServeWebSocket`: every request gets the session as it is now (cache or database), never the one of
            // the connect: a revoked session fails, a changed one carries its new roles.
            Message::Text(t) => match app.session(&conn.token).await {
                Ok(fresh) => action(&app, &fresh, &conn, &t).await,
                Err(e) => session_error_reply(&conn, &t, e),
            },
            Message::Close(_) => break,
            _ => {}
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
    id: String,
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

/// `ServeWebSocket` when the connection's session is gone or invalid: the error reply, its detail wiped.
fn session_error_reply(conn: &Conn, text: &str, e: crate::app::AppError) {
    let seq_reply = serde_json::from_str::<Value>(text).ok().and_then(|v| v["seq"].as_i64()).unwrap_or(0);
    let error = WsReplyError { id: e.id, message: e.message, detailed_error: "", status_code: e.status_code };
    let msg = WsReply { status: "FAIL", seq_reply, data: None, error: Some(error) };
    conn.send(Out::Reply(serde_json::to_string(&msg).unwrap_or_default()));
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
            let error = WsReplyError { id: id.to_string(), message, detailed_error: "", status_code };
            WsReply { status: "FAIL", seq_reply, data: None, error: Some(error) }
        }
    };
    let msg = serde_json::to_string(&msg).unwrap_or_default();
    conn.send(Out::Reply(msg));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(s: Session) -> (Conn, mpsc::Receiver<Out>) {
        let (tx, rx) = mpsc::channel(SEND_QUEUE);
        (Conn::new(1, Arc::new(s), tx), rx)
    }

    #[test]
    fn stale_after_invalidation_or_expiry() {
        let now = now_ms();
        let (c, _rx) = conn(Session { user_id: "u".into(), token: "t".into(), expires_at: now + 60_000, ..Default::default() });
        assert!(!c.is_stale(now));
        // InvalidateCache
        c.generation.fetch_add(1, Ordering::AcqRel);
        assert!(c.is_stale(now));
        c.loaded.store(1, Ordering::Release);
        assert!(!c.is_stale(now));
        // Past its expiry; 0 never expires.
        assert!(c.is_stale(now + 120_000));
        c.expires_at.store(0, Ordering::Relaxed);
        assert!(!c.is_stale(now + 120_000));
    }

    #[test]
    fn session_flags() {
        let mut s = Session { user_id: "u".into(), roles: "system_user system_admin".into(), ..Default::default() };
        s.props.0.insert("is_guest".into(), "true".into());
        let (c, _rx) = conn(s);
        assert!(c.admin.load(Ordering::Relaxed) && c.guest.load(Ordering::Relaxed));
        c.set_session(Arc::new(Session { user_id: "u".into(), roles: "system_user".into(), ..Default::default() }));
        assert!(!c.admin.load(Ordering::Relaxed) && !c.guest.load(Ordering::Relaxed));
    }

    #[test]
    fn guest_subjects() {
        assert_eq!(guest_subject("user_updated", &json!({"user": {"id": "a"}})).as_deref(), Some("a"));
        assert_eq!(guest_subject("new_user", &json!({"user_id": "b"})).as_deref(), Some("b"));
        assert_eq!(guest_subject("user_updated", &json!({})).as_deref(), Some(""));
        assert_eq!(guest_subject("posted", &json!({"user_id": "b"})), None);
    }
}

//! Websocket connections and event broadcast, as Mattermost's platform hub does it on one node: an event is
//! serialised once, each connection adds its own `seq`.

// Based on Mattermost server/channels/app/platform/web_hub.go and web_conn.go, Copyright (c) 2015-present
// Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::HashMap;
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

use crate::app::{App, State, new_id, now_ms, token_from};
use crate::model::{Status, status};

#[derive(Default)]
pub struct Hub {
    conns: RwLock<HashMap<u64, Conn>>,
    next: AtomicU64,
}

struct Conn {
    user_id: String,
    tx: mpsc::UnboundedSender<Arc<str>>,
}

/// `model.WebsocketBroadcast`, the part clients see.
#[derive(Serialize, Default)]
pub struct Broadcast {
    pub omit_users: Option<HashMap<String, bool>>,
    pub user_id: String,
    pub channel_id: String,
    pub team_id: String,
    pub connection_id: String,
    pub omit_connection_id: String,
}

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
    fn add(&self, user_id: &str, tx: mpsc::UnboundedSender<Arc<str>>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.conns.write().unwrap().insert(id, Conn { user_id: user_id.into(), tx });
        id
    }
    /// Removes a connection; true when it was the user's last one.
    fn remove(&self, id: u64) -> bool {
        let mut c = self.conns.write().unwrap();
        let Some(gone) = c.remove(&id) else { return false };
        !c.values().any(|x| x.user_id == gone.user_id)
    }

    pub fn to_user(&self, user_id: &str, event: &str, data: Value) {
        let b = Broadcast { user_id: user_id.into(), ..Default::default() };
        let p = prefix(event, &data, &b);
        for c in self.conns.read().unwrap().values().filter(|c| c.user_id == user_id) {
            let _ = c.tx.send(p.clone());
        }
    }

    /// To every connection of `members`, the users of a channel.
    pub fn to_channel(&self, channel_id: &str, members: &[String], event: &str, data: Value) {
        let b = Broadcast { channel_id: channel_id.into(), ..Default::default() };
        let p = prefix(event, &data, &b);
        let set: std::collections::HashSet<&str> = members.iter().map(String::as_str).collect();
        for c in self.conns.read().unwrap().values().filter(|c| set.contains(c.user_id.as_str())) {
            let _ = c.tx.send(p.clone());
        }
    }
}

/// The token of the upgrade request, if any: a client may also send it in an `authentication_challenge`.
pub struct MaybeToken(Option<String>);

impl FromRequestParts<State> for MaybeToken {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &State) -> Result<MaybeToken, Self::Rejection> {
        Ok(MaybeToken(token_from(parts)))
    }
}

pub async fn connect(ws: WebSocketUpgrade, MaybeToken(token): MaybeToken, St(app): St<State>) -> Response {
    ws.on_upgrade(move |socket| serve(app, socket, token))
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
    let Some(session) = (match token {
        Some(t) => app.session(&t).await.ok().filter(|s| !s.is_expired(now_ms())),
        None => None,
    }) else {
        let _ = sink.close().await;
        return;
    };
    let user_id = session.user_id.clone();
    let (tx, mut rx) = mpsc::unbounded_channel::<Arc<str>>();
    let conn = app.hub.add(&user_id, tx);
    let connection_id = new_id();

    let writer = tokio::spawn(async move {
        let mut seq: i64 = 0;
        let mut ping = tokio::time::interval(Duration::from_secs(60));
        ping.tick().await;
        loop {
            tokio::select! {
                m = rx.recv() => {
                    let Some(p) = m else { break };
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
    });

    let hello = json!({"connection_id": connection_id, "server_version": crate::SERVER_VERSION});
    app.hub.to_user(&user_id, "hello", hello);
    set_online(&app, &user_id).await;

    while let Some(Ok(m)) = stream.next().await {
        if let Message::Close(_) = m {
            break;
        }
    }
    writer.abort();
    if app.hub.remove(conn) {
        set_status(&app, &user_id, status::OFFLINE).await;
    }
}

async fn set_online(app: &App, user_id: &str) {
    set_status(app, user_id, status::ONLINE).await;
}

/// `SetStatusOnline` / `SetStatusOffline` for an automatic (not manual) status: saved and sent to the user only
/// when it changes; a manual status (dnd, away) stays.
async fn set_status(app: &App, user_id: &str, to: &str) {
    let now = now_ms();
    let current = crate::api::statuses(app, &[user_id.to_string()]).await.ok().and_then(|mut v| v.pop());
    let mut s = current.unwrap_or_else(|| Status::offline(user_id));
    if s.manual && to == status::ONLINE {
        return;
    }
    let changed = s.status != to;
    s.last_activity_at = now;
    if changed {
        s.prev_status = std::mem::replace(&mut s.status, to.to_string());
    }
    app.statuses.write().unwrap().insert(user_id.to_string(), s.clone());
    if changed {
        let _ = sqlx::query(
            "INSERT INTO Status (UserId, Status, Manual, LastActivityAt, DNDEndTime, PrevStatus) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (UserId) DO UPDATE SET Status = EXCLUDED.Status, Manual = EXCLUDED.Manual,
             LastActivityAt = EXCLUDED.LastActivityAt, DNDEndTime = EXCLUDED.DNDEndTime, PrevStatus = EXCLUDED.PrevStatus",
        )
        .bind(&s.user_id)
        .bind(&s.status)
        .bind(s.manual)
        .bind(s.last_activity_at)
        .bind(s.dnd_end_time)
        .bind(&s.prev_status)
        .execute(&app.db)
        .await;
        app.hub.to_user(user_id, "status_change", json!({"status": s.status, "user_id": user_id}));
    }
}

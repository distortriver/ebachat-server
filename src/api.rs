//! REST API v4: the first slice, the endpoints production calls most.

// Based on Mattermost server/channels/api4 (status.go, user.go, channel.go, post.go), app and sqlstore,
// Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query, State as St};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::types::Json as SqlJson;

use crate::app::{ApiResult, App, AppError, Auth, State, is_valid_id, new_id, now_ms};
use crate::db::{self, Table};
use crate::model::{JsonText, Post, Status, User};

pub fn router(app: State) -> Router {
    Router::new()
        .route("/api/v4/system/ping", get(|| async { Json(json!({"status": "OK"})) }))
        .route("/api/v4/users/me", get(get_me))
        .route("/api/v4/users/ids", post(users_by_ids))
        .route("/api/v4/users/status/ids", post(statuses_by_ids))
        .route("/api/v4/channels/members/{user_id}/view", post(view_channel))
        .route("/api/v4/channels/{channel_id}/posts", get(posts_for_channel))
        .route("/api/v4/posts", post(create_post))
        .route("/api/v4/websocket", get(crate::hub::connect))
        .with_state(app)
}

// ---------- users ----------

async fn get_me(St(app): St<State>, Auth(s): Auth) -> ApiResult<Json<User>> {
    let mut u: User = db::get(&app.db, &s.user_id).await?.ok_or_else(AppError::forbidden)?;
    u.sanitize(true, true, true, true, true);
    Ok(Json(u))
}

/// `SqlUserStore.GetProfileByIds`: users with their bot fields.
async fn profiles(app: &App, ids: &[String], since: i64) -> ApiResult<Vec<User>> {
    let sql = format!("{} WHERE id = ANY($1) AND updateat > $2", User::SELECT);
    let mut users: Vec<User> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(ids).bind(since).fetch_all(&app.db).await?;
    let bots: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT UserId, COALESCE(Description, ''), COALESCE(LastIconUpdate, 0) FROM Bots WHERE UserId = ANY($1)",
    )
    .bind(ids)
    .fetch_all(&app.db)
    .await?;
    let bots: HashMap<_, _> = bots.into_iter().map(|(id, d, l)| (id, (d, l))).collect();
    for u in &mut users {
        if let Some((d, l)) = bots.get(&u.id) {
            (u.is_bot, u.bot_description, u.bot_last_icon_update) = (true, d.clone(), *l);
        }
    }
    Ok(users)
}

#[derive(Deserialize)]
struct SinceQuery {
    since: Option<i64>,
}

async fn users_by_ids(
    St(app): St<State>,
    Auth(s): Auth,
    Query(q): Query<SinceQuery>,
    Json(ids): Json<Vec<String>>,
) -> ApiResult<Json<Vec<User>>> {
    if ids.is_empty() {
        return Err(AppError::invalid_param("user_ids"));
    }
    let mut users = profiles(&app, &ids, q.since.unwrap_or(-1)).await?;
    let admin = s.is_system_admin();
    for u in &mut users {
        u.sanitize_profile(admin);
    }
    Ok(Json(users))
}

// ---------- statuses ----------

/// `PlatformService.GetUserStatusesByIds`: the cache, then the database, then offline for the rest.
pub async fn statuses(app: &App, ids: &[String]) -> ApiResult<Vec<Status>> {
    let mut out = Vec::with_capacity(ids.len());
    let mut missing = vec![];
    {
        let cache = app.statuses.read().unwrap();
        for id in ids {
            match cache.get(id) {
                Some(s) => out.push(s.clone()),
                None => missing.push(id.clone()),
            }
        }
    }
    if !missing.is_empty() {
        let sql = format!("{} WHERE userid = ANY($1)", Status::SELECT);
        let found: Vec<Status> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(&missing).fetch_all(&app.db).await?;
        let mut cache = app.statuses.write().unwrap();
        for s in &found {
            cache.entry(s.user_id.clone()).or_insert_with(|| s.clone());
        }
        missing.retain(|id| !found.iter().any(|s| &s.user_id == id));
        out.extend(found);
        out.extend(missing.iter().map(|id| Status::offline(id)));
    }
    Ok(out)
}

async fn statuses_by_ids(St(app): St<State>, _: Auth, Json(ids): Json<Vec<String>>) -> ApiResult<Json<Vec<Status>>> {
    if ids.is_empty() || ids.iter().any(|i| i.len() != 26) {
        return Err(AppError::invalid_param("user_ids"));
    }
    Ok(Json(statuses(&app, &ids).await?))
}

// ---------- channel view ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct ChannelView {
    channel_id: String,
    prev_channel_id: String,
}

async fn view_channel(
    St(app): St<State>,
    Auth(s): Auth,
    Path(user_id): Path<String>,
    Json(view): Json<ChannelView>,
) -> ApiResult<impl IntoResponse> {
    let user_id = if user_id == "me" { s.user_id.clone() } else { user_id };
    if user_id != s.user_id && !s.is_system_admin() {
        return Err(AppError::forbidden());
    }
    for (name, id) in
        [("channel_view.channel_id", &view.channel_id), ("channel_view.prev_channel_id", &view.prev_channel_id)]
    {
        if !id.is_empty() && !is_valid_id(id) {
            return Err(AppError::invalid_param(name));
        }
    }
    // SetActiveChannel
    if let Some(st) = app.statuses.write().unwrap().get_mut(&user_id) {
        st.active_channel = view.channel_id.clone();
        st.last_activity_at = now_ms();
    }
    let ids: Vec<String> = [view.channel_id, view.prev_channel_id].into_iter().filter(|c| !c.is_empty()).collect();
    let times = if ids.is_empty() { BTreeMap::new() } else { mark_viewed(&app, &ids, &user_id).await? };
    Ok(Json(json!({"status": "OK", "last_viewed_at_times": times})))
}

/// `App.MarkChannelsAsViewed` with collapsed threads on (no thread updates) and push notifications off.
async fn mark_viewed(app: &App, ids: &[String], user_id: &str) -> ApiResult<BTreeMap<String, i64>> {
    // SqlChannelStore.GetChannelsWithUnreadsAndWithMentions
    let rows: Vec<(String, i64, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT Channels.Id, COALESCE(Channels.TotalMsgCount, 0), COALESCE(Channels.LastPostAt, 0),
                COALESCE(ChannelMembers.MsgCount, 0), COALESCE(ChannelMembers.MentionCount, 0),
                COALESCE(ChannelMembers.LastViewedAt, 0)
         FROM ChannelMembers INNER JOIN Channels ON ChannelMembers.ChannelId = Channels.Id
         WHERE ChannelMembers.ChannelId = ANY($1) AND ChannelMembers.UserId = $2",
    )
    .bind(ids)
    .bind(user_id)
    .fetch_all(&app.db)
    .await?;
    let mut times = BTreeMap::new();
    let mut to_view = vec![];
    for (id, total, last_post, msg, mentions, last_viewed) in rows {
        if total - msg > 0 || mentions > 0 {
            to_view.push(id.clone());
        }
        times.insert(id, last_post.max(last_viewed));
    }
    if to_view.is_empty() {
        return Ok(times);
    }
    // SqlChannelStore.UpdateLastViewedAt
    sqlx::query(
        "WITH c AS (SELECT Id, LastPostAt, TotalMsgCount, TotalMsgCountRoot FROM Channels WHERE Id = ANY($1)),
         updated AS (UPDATE ChannelMembers cm SET MentionCount = 0, MentionCountRoot = 0, UrgentMentionCount = 0,
             MsgCount = greatest(cm.MsgCount, c.TotalMsgCount), MsgCountRoot = greatest(cm.MsgCountRoot, c.TotalMsgCountRoot),
             LastViewedAt = greatest(cm.LastViewedAt, c.LastPostAt), LastUpdateAt = greatest(cm.LastViewedAt, c.LastPostAt)
             FROM c WHERE cm.UserId = $2 AND c.Id = cm.ChannelId)
         SELECT Id, LastPostAt FROM c",
    )
    .bind(&to_view)
    .bind(user_id)
    .execute(&app.db)
    .await?;
    app.hub.to_user(user_id, "multiple_channels_viewed", json!({"channel_times": times}));
    Ok(times)
}

// ---------- posts ----------

async fn is_member(app: &App, channel_id: &str, user_id: &str) -> ApiResult<bool> {
    let r: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM ChannelMembers WHERE ChannelId = $1 AND UserId = $2")
        .bind(channel_id)
        .bind(user_id)
        .fetch_optional(&app.db)
        .await?;
    Ok(r.is_some())
}

#[derive(Deserialize)]
struct PostsQuery {
    since: Option<i64>,
}

/// A post as `PreparePostForClient` sends a plain one: empty metadata. Files, reactions, emojis, embeds and
/// priority are not filled in yet.
fn for_client(mut p: Post) -> Post {
    p.metadata = Some(json!({}));
    p
}

async fn posts_for_channel(
    St(app): St<State>,
    Auth(s): Auth,
    Path(channel_id): Path<String>,
    Query(q): Query<PostsQuery>,
) -> ApiResult<Json<Value>> {
    if !is_valid_id(&channel_id) {
        return Err(AppError::invalid_param("channel_id"));
    }
    if !is_member(&app, &channel_id, &s.user_id).await? {
        return Err(AppError::forbidden());
    }
    let Some(since) = q.since else {
        return Err(AppError::new(StatusCode::NOT_IMPLEMENTED, "ebachat.not_implemented", "Only ?since= for now."));
    };
    // SqlPostStore.GetPostsSince, without collapsed threads.
    let cols = Post::COLUMNS.join(", ");
    let sql = format!(
        "WITH cte AS (SELECT {cols} FROM Posts WHERE UpdateAt > $1 AND ChannelId = $2 LIMIT 1000)
         (SELECT {cols} FROM cte) UNION (SELECT {cols} FROM Posts p1 WHERE id IN (SELECT rootid FROM cte))
         ORDER BY CreateAt DESC"
    );
    let posts: Vec<Post> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(since).bind(&channel_id).fetch_all(&app.db).await?;
    let order: Vec<&str> = posts.iter().filter(|p| p.update_at > since).map(|p| p.id.as_str()).collect();
    let order: Vec<String> = order.into_iter().map(str::to_string).collect();
    let map: BTreeMap<String, Post> = posts.into_iter().map(|p| (p.id.clone(), for_client(p))).collect();
    Ok(Json(json!({
        "order": order, "posts": map, "next_post_id": "", "prev_post_id": "", "first_inaccessible_post_time": 0
    })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NewPost {
    channel_id: String,
    root_id: String,
    message: String,
    props: serde_json::Map<String, Value>,
    file_ids: Vec<String>,
    pending_post_id: String,
}

async fn create_post(St(app): St<State>, Auth(s): Auth, Json(np): Json<NewPost>) -> ApiResult<impl IntoResponse> {
    if !is_valid_id(&np.channel_id) {
        return Err(AppError::invalid_param("post.channel_id"));
    }
    let channel: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT TeamId, Type::text, DisplayName, Name FROM Channels WHERE Id = $1 AND DeleteAt = 0")
            .bind(&np.channel_id)
            .fetch_optional(&app.db)
            .await?;
    let Some((team_id, ctype, display_name, name)) = channel else { return Err(AppError::forbidden()) };
    if !is_member(&app, &np.channel_id, &s.user_id).await? {
        return Err(AppError::forbidden());
    }
    let now = now_ms();
    let post = Post {
        id: new_id(),
        create_at: now,
        update_at: now,
        user_id: s.user_id.clone(),
        channel_id: np.channel_id.clone(),
        root_id: np.root_id.clone(),
        message: np.message,
        props: SqlJson(np.props),
        file_ids: JsonText(np.file_ids),
        filenames: JsonText(vec![]),
        pending_post_id: np.pending_post_id,
        ..Default::default()
    };
    // SqlPostStore.Save: the post and the channel's counters, in one transaction.
    let mut tx = app.db.begin().await?;
    post.bind(sqlx::query(Post::INSERT)).execute(&mut *tx).await?;
    let root = post.root_id.is_empty();
    sqlx::query(
        "UPDATE Channels SET LastPostAt = GREATEST($1, LastPostAt),
             LastRootPostAt = CASE WHEN $2 THEN GREATEST($1, LastRootPostAt) ELSE LastRootPostAt END,
             TotalMsgCount = TotalMsgCount + 1,
             TotalMsgCountRoot = TotalMsgCountRoot + CASE WHEN $2 THEN 1 ELSE 0 END
         WHERE Id = $3",
    )
    .bind(now)
    .bind(root)
    .bind(&post.channel_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let members: Vec<(String,)> = sqlx::query_as("SELECT UserId FROM ChannelMembers WHERE ChannelId = $1")
        .bind(&post.channel_id)
        .fetch_all(&app.db)
        .await?;
    let members: Vec<String> = members.into_iter().map(|(u,)| u).collect();
    let sender: Option<(String,)> =
        sqlx::query_as("SELECT Username FROM Users WHERE Id = $1").bind(&s.user_id).fetch_optional(&app.db).await?;
    let out = for_client(post);
    let data = json!({
        "channel_display_name": display_name, "channel_name": name, "channel_type": ctype,
        "post": serde_json::to_string(&out).unwrap(), "sender_name": format!("@{}", sender.map(|s| s.0).unwrap_or_default()),
        "set_online": true, "team_id": team_id,
    });
    app.hub.to_channel(&out.channel_id, &members, "posted", data);
    mark_viewed(&app, std::slice::from_ref(&out.channel_id), &s.user_id).await?;
    Ok((StatusCode::CREATED, Json(out)))
}

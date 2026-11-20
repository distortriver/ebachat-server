//! Sidebar categories (`/users/{user_id}/teams/{team_id}/channels/categories`): favorites, channels, direct
//! messages and the user's own, with the channels in each; channels not placed anywhere fall into channels or
//! direct messages.
//!
//! Contract: [`App::create_initial_sidebar`].

// Based on Mattermost server/channels/api4/channel_category.go, app/channel_category.go and
// store/sqlstore/channel_store_categories.go, Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::HashMap;

use axum::extract::{Path, State as St};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{Value, json};

use crate::app::{ApiResult, App, AppError, Auth, Body, State, is_valid_id, new_id};

use crate::hub::Broadcast;
use crate::model::{Session, SidebarCategory, category};
use crate::perm;

pub fn routes() -> Router<State> {
    let base = "/api/v4/users/{user_id}/teams/{team_id}/channels/categories";
    Router::new()
        .route(base, get(get_categories).post(create_category).put(update_categories))
        .route(&format!("{base}/order"), get(get_order).put(update_order))
        .route(&format!("{base}/{{category_id}}"), get(get_category).put(update_category).delete(delete_category))
}

const SORT_DISTANCE: i64 = 10;

/// `OrderedSidebarCategories`.
#[derive(Serialize)]
pub struct Ordered {
    pub categories: Vec<SidebarCategory>,
    pub order: Vec<String>,
}

fn categories_error(status: axum::http::StatusCode) -> AppError {
    AppError::new(status, "app.channel.sidebar_categories.app_error", "Failed to insert record to database.")
}

impl App {
    /// `createInitialSidebarCategories`: favorites (with the favorite-channel preferences), channels and direct
    /// messages, those missing.
    pub async fn create_initial_sidebar(&self, user_id: &str, team_id: &str) -> ApiResult<()> {
        let mut tx = self.db.begin().await?;
        let existing: Vec<String> = sqlx::query_scalar(
            "SELECT Type FROM SidebarCategories WHERE UserId = $1 AND TeamId = $2
             AND Type IN ('favorites', 'channels', 'direct_messages')",
        )
        .bind(user_id)
        .bind(team_id)
        .fetch_all(&mut *tx)
        .await?;
        let has = |t: &str| existing.iter().any(|e| e == t);
        let mut rows: Vec<(String, i64, &str, &str, &str)> = vec![];
        if !has(category::FAVORITES) {
            let id = format!("{}_{user_id}_{team_id}", category::FAVORITES);
            // migrateFavoritesToSidebarT
            let favorites: Vec<String> = sqlx::query_scalar(
                "SELECT Preferences.Name FROM Preferences
                 JOIN Channels ON Preferences.Name = Channels.Id
                 JOIN ChannelMembers ON Preferences.Name = ChannelMembers.ChannelId AND Preferences.UserId = ChannelMembers.UserId
                 WHERE Preferences.UserId = $1 AND Preferences.Category = 'favorite_channel' AND Preferences.Value = 'true'
                 AND (Channels.TeamId = $2 OR Channels.TeamId = '')
                 ORDER BY Channels.DisplayName, Channels.Name ASC",
            )
            .bind(user_id)
            .bind(team_id)
            .fetch_all(&mut *tx)
            .await?;
            for (i, channel_id) in favorites.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO SidebarChannels (ChannelId, UserId, CategoryId, SortOrder) VALUES ($1, $2, $3, $4)",
                )
                .bind(channel_id)
                .bind(user_id)
                .bind(&id)
                .bind(i as i64 * SORT_DISTANCE)
                .execute(&mut *tx)
                .await?;
            }
            rows.push((id, 0, category::SORT_DEFAULT, category::FAVORITES, "Favorites"));
        }
        if !has(category::CHANNELS) {
            let id = format!("{}_{user_id}_{team_id}", category::CHANNELS);
            rows.push((id, SORT_DISTANCE, category::SORT_DEFAULT, category::CHANNELS, "Channels"));
        }
        if !has(category::DIRECT_MESSAGES) {
            let id = format!("{}_{user_id}_{team_id}", category::DIRECT_MESSAGES);
            rows.push((id, 2 * SORT_DISTANCE, category::SORT_RECENT, category::DIRECT_MESSAGES, "Direct Messages"));
        }
        for (id, order, sorting, kind, name) in rows {
            sqlx::query(
                "INSERT INTO SidebarCategories (Id, UserId, TeamId, SortOrder, Sorting, Type, DisplayName, Muted, Collapsed)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, false, false)",
            )
            .bind(id)
            .bind(user_id)
            .bind(team_id)
            .bind(order)
            .bind(sorting)
            .bind(kind)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// `getOrphanedSidebarChannels`: the user's channels (of the team) and DMs/GMs in no category of the team.
    async fn orphaned_sidebar_channels(
        &self,
        user_id: &str,
        team_id: &str,
        channels: bool,
        dms: bool,
    ) -> ApiResult<Vec<(String, String)>> {
        if !channels && !dms {
            return Ok(vec![]);
        }
        let mut types = vec![];
        if dms {
            types.push("Channels.Type IN ('D', 'G')".to_string());
        }
        if channels {
            types.push("(Channels.Type IN ('O', 'P') AND Channels.TeamId = $2)".to_string());
        }
        let sql = format!(
            "SELECT Id, Channels.Type::text FROM ChannelMembers LEFT JOIN Channels ON Channels.Id = ChannelMembers.ChannelId
             WHERE ChannelMembers.UserId = $1 AND ({}) AND Channels.DeleteAt = 0
             AND NOT EXISTS (SELECT 1 FROM SidebarChannels JOIN SidebarCategories ON SidebarChannels.CategoryId = SidebarCategories.Id
                 WHERE SidebarChannels.ChannelId = ChannelMembers.ChannelId AND SidebarCategories.UserId = $1
                 AND SidebarCategories.TeamId = $2)
             ORDER BY DisplayName ASC",
            types.join(" OR ")
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(user_id).bind(team_id).fetch_all(&self.db).await?)
    }

    /// `completePopulatingCategoriesT`.
    async fn complete_categories(&self, user_id: &str, team_id: &str, cats: &mut [SidebarCategory]) -> ApiResult<()> {
        let chans = cats.iter().position(|c| c.r#type == category::CHANNELS);
        let dms = cats.iter().position(|c| c.r#type == category::DIRECT_MESSAGES);
        for (id, kind) in self.orphaned_sidebar_channels(user_id, team_id, chans.is_some(), dms.is_some()).await? {
            match (kind.as_str(), chans, dms) {
                ("O" | "P", Some(i), _) => cats[i].channel_ids.push(id),
                ("D" | "G", _, Some(i)) => cats[i].channel_ids.push(id),
                _ => {}
            }
        }
        Ok(())
    }

    /// `getSidebarCategoriesT`.
    async fn sidebar_categories(&self, user_id: &str, team_id: &str) -> ApiResult<Ordered> {
        let sql = format!(
            "SELECT {}, SidebarChannels.ChannelId AS joined_channel FROM SidebarCategories
             LEFT JOIN SidebarChannels ON SidebarChannels.CategoryId = SidebarCategories.Id
             WHERE SidebarCategories.UserId = $1 AND SidebarCategories.TeamId = $2
             ORDER BY SidebarCategories.SortOrder ASC, SidebarChannels.SortOrder ASC",
            crate::db::cols::<SidebarCategory>("SidebarCategories")
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(user_id).bind(team_id).fetch_all(&self.db).await?;
        let mut cats: Vec<SidebarCategory> = vec![];
        let mut order: Vec<String> = vec![];
        for row in rows {
            let c: SidebarCategory = sqlx::FromRow::from_row(&row)?;
            let joined: Option<String> = sqlx::Row::try_get(&row, "joined_channel")?;
            let i = match cats.iter().position(|x| x.id == c.id) {
                Some(i) => i,
                None => {
                    order.push(c.id.clone());
                    cats.push(c);
                    cats.len() - 1
                }
            };
            if let Some(ch) = joined {
                cats[i].channel_ids.push(ch);
            }
        }
        self.complete_categories(user_id, team_id, &mut cats).await?;
        Ok(Ordered { categories: cats, order })
    }

    /// `GetSidebarCategoriesForTeamForUser`: created on the first read.
    pub async fn sidebar_for_team_for_user(&self, user_id: &str, team_id: &str) -> ApiResult<Ordered> {
        let o = self.sidebar_categories(user_id, team_id).await?;
        if !o.categories.is_empty() {
            return Ok(o);
        }
        self.create_initial_sidebar(user_id, team_id).await?;
        self.sidebar_categories(user_id, team_id).await
    }

    /// `getSidebarCategoryT`.
    async fn sidebar_category(&self, id: &str) -> ApiResult<Option<SidebarCategory>> {
        let sql = format!(
            "SELECT {}, SidebarChannels.ChannelId AS joined_channel FROM SidebarCategories
             LEFT JOIN SidebarChannels ON SidebarChannels.CategoryId = SidebarCategories.Id
             WHERE SidebarCategories.Id = $1 ORDER BY SidebarChannels.SortOrder ASC",
            crate::db::cols::<SidebarCategory>("SidebarCategories")
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id).fetch_all(&self.db).await?;
        let mut out: Option<SidebarCategory> = None;
        for row in rows {
            let joined: Option<String> = sqlx::Row::try_get(&row, "joined_channel")?;
            let c = out.get_or_insert(sqlx::FromRow::from_row(&row)?);
            if let Some(ch) = joined {
                c.channel_ids.push(ch);
            }
        }
        let Some(mut c) = out else { return Ok(None) };
        let (user, team) = (c.user_id.clone(), c.team_id.clone());
        let (chans, dms) = (c.r#type == category::CHANNELS, c.r#type == category::DIRECT_MESSAGES);
        for (id, _) in self.orphaned_sidebar_channels(&user, &team, chans, dms).await? {
            c.channel_ids.push(id);
        }
        Ok(Some(c))
    }

    async fn category_order(&self, user_id: &str, team_id: &str) -> ApiResult<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT Id FROM SidebarCategories WHERE UserId = $1 AND TeamId = $2 ORDER BY SortOrder ASC",
        )
        .bind(user_id)
        .bind(team_id)
        .fetch_all(&self.db)
        .await?)
    }

    async fn write_category_order(tx: &mut sqlx::PgConnection, order: &[String]) -> ApiResult<()> {
        for (i, id) in order.iter().enumerate() {
            sqlx::query("UPDATE SidebarCategories SET SortOrder = $1 WHERE Id = $2")
                .bind(i as i64 * SORT_DISTANCE)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        Ok(())
    }

    /// `CreateSidebarCategory`: after favorites if they are first, else first.
    async fn create_category(&self, user_id: &str, team_id: &str, new: &SidebarCategory) -> ApiResult<SidebarCategory> {
        let existing = self.sidebar_categories(user_id, team_id).await?;
        if existing.categories.is_empty() {
            return Err(categories_error(axum::http::StatusCode::NOT_FOUND));
        }
        let id = new_id();
        let mut order = existing.order.clone();
        let sort_order = if existing.categories[0].r#type == category::FAVORITES {
            order.insert(1, id.clone());
            SORT_DISTANCE
        } else {
            order.insert(0, id.clone());
            0
        };
        let mut tx = self.db.begin().await?;
        sqlx::query(
            "INSERT INTO SidebarCategories (Id, UserId, TeamId, SortOrder, Sorting, Type, DisplayName, Muted, Collapsed)
             VALUES ($1, $2, $3, $4, $5, 'custom', $6, $7, false)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(team_id)
        .bind(SORT_DISTANCE * order.len() as i64)
        .bind(&new.sorting)
        .bind(&new.display_name)
        .bind(new.muted)
        .execute(&mut *tx)
        .await?;
        if !new.channel_ids.is_empty() {
            sqlx::query(
                "DELETE FROM SidebarChannels USING SidebarCategories WHERE SidebarChannels.CategoryId = SidebarCategories.Id
                 AND SidebarChannels.UserId = $1 AND SidebarChannels.ChannelId = ANY($2) AND SidebarCategories.TeamId = $3",
            )
            .bind(user_id)
            .bind(&new.channel_ids)
            .bind(team_id)
            .execute(&mut *tx)
            .await?;
            for (i, ch) in new.channel_ids.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO SidebarChannels (ChannelId, UserId, CategoryId, SortOrder) VALUES ($1, $2, $3, $4)",
                )
                .bind(ch)
                .bind(user_id)
                .bind(&id)
                .bind(i as i64 * SORT_DISTANCE)
                .execute(&mut *tx)
                .await?;
            }
        }
        Self::write_category_order(&mut tx, &order).await?;
        tx.commit().await?;
        Ok(SidebarCategory {
            id,
            user_id: user_id.into(),
            team_id: team_id.into(),
            sort_order,
            sorting: new.sorting.clone(),
            r#type: category::CUSTOM.into(),
            display_name: new.display_name.clone(),
            muted: new.muted,
            collapsed: false,
            channel_ids: new.channel_ids.clone(),
        })
    }

    /// `UpdateSidebarCategories`: what may change of each, their channels rewritten, favorites kept in the
    /// preferences; returns (updated, original).
    async fn update_categories(
        &self,
        user_id: &str,
        team_id: &str,
        cats: &[SidebarCategory],
    ) -> ApiResult<(Vec<SidebarCategory>, Vec<SidebarCategory>)> {
        let mut updated = vec![];
        let mut original = vec![];
        for c in cats {
            let src = self
                .sidebar_category(&c.id)
                .await?
                .ok_or_else(|| categories_error(axum::http::StatusCode::INTERNAL_SERVER_ERROR))?;
            let mut d = c.clone();
            d.user_id = src.user_id.clone();
            d.team_id = src.team_id.clone();
            d.sort_order = src.sort_order;
            d.r#type = src.r#type.clone();
            d.muted = src.muted;
            if d.r#type != category::CUSTOM {
                d.display_name = src.display_name.clone();
            }
            if d.r#type != category::DIRECT_MESSAGES {
                d.channel_ids = c.channel_ids.clone();
                d.muted = c.muted;
            } else {
                d.channel_ids = vec![];
            }
            updated.push(d);
            original.push(src);
        }
        let mut tx = self.db.begin().await?;
        let mut sorted: Vec<&SidebarCategory> = updated.iter().collect();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));
        for d in sorted {
            sqlx::query(
                "UPDATE SidebarCategories SET DisplayName = $1, Sorting = $2, Muted = $3, Collapsed = $4 WHERE Id = $5",
            )
            .bind(&d.display_name)
            .bind(&d.sorting)
            .bind(d.muted)
            .bind(d.collapsed)
            .bind(&d.id)
            .execute(&mut *tx)
            .await?;
        }
        let ids: Vec<String> = cats.iter().map(|c| c.id.clone()).collect();
        sqlx::query("DELETE FROM SidebarChannels WHERE CategoryId = ANY($1)").bind(&ids).execute(&mut *tx).await?;
        for c in cats {
            if c.r#type == category::DIRECT_MESSAGES {
                continue;
            }
            for (i, ch) in c.channel_ids.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO SidebarChannels (ChannelId, UserId, CategoryId, SortOrder) VALUES ($1, $2, $3, $4)
                     ON CONFLICT (ChannelId, UserId, CategoryId) DO UPDATE SET SortOrder = excluded.SortOrder",
                )
                .bind(ch)
                .bind(user_id)
                .bind(&c.id)
                .bind(i as i64 * SORT_DISTANCE)
                .execute(&mut *tx)
                .await?;
            }
        }
        for (i, c) in cats.iter().enumerate() {
            let names = if c.r#type == category::FAVORITES { &original[i].channel_ids } else { &c.channel_ids };
            sqlx::query(
                "DELETE FROM Preferences WHERE UserId = $1 AND Name = ANY($2) AND Category = 'favorite_channel'",
            )
            .bind(user_id)
            .bind(names)
            .execute(&mut *tx)
            .await?;
            if c.r#type == category::FAVORITES {
                for ch in &c.channel_ids {
                    sqlx::query(
                        "INSERT INTO Preferences (UserId, Category, Name, Value) VALUES ($1, 'favorite_channel', $2, 'true')
                         ON CONFLICT (UserId, Category, Name) DO UPDATE SET Value = 'true'",
                    )
                    .bind(user_id)
                    .bind(ch)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        tx.commit().await?;
        self.complete_categories(user_id, team_id, &mut updated).await?;
        Ok((updated, original))
    }

    /// `muteChannelsForUpdatedCategories`: channels follow the mute state of the category they are in.
    async fn mute_for_categories(
        &self,
        user_id: &str,
        updated: &[SidebarCategory],
        original: &[SidebarCategory],
    ) -> ApiResult<()> {
        let mut mute = vec![];
        let mut unmute = vec![];
        for (u, o) in updated.iter().zip(original) {
            if u.muted && !o.muted {
                mute.extend(u.channel_ids.iter().cloned());
            } else if !u.muted && o.muted {
                unmute.extend(u.channel_ids.iter().cloned());
            }
        }
        let to_map = |cats: &[SidebarCategory]| {
            let mut m: HashMap<String, String> = HashMap::new();
            for c in cats {
                for ch in &c.channel_ids {
                    m.insert(ch.clone(), c.id.clone());
                }
            }
            m
        };
        let (new_map, old_map) = (to_map(updated), to_map(original));
        let by_id = |cats: &[SidebarCategory], id: &str| cats.iter().find(|c| c.id == id).map(|c| c.muted);
        for (ch, from) in &old_map {
            let Some(to) = new_map.get(ch) else { continue };
            if to == from {
                continue;
            }
            let (Some(to_muted), Some(from_muted)) = (by_id(updated, to), by_id(original, from)) else { continue };
            if to_muted && !from_muted {
                mute.push(ch.clone());
            } else if !to_muted && from_muted {
                unmute.push(ch.clone());
            }
        }
        if !mute.is_empty() {
            self.set_channels_muted(&mute, user_id, true).await?;
        }
        if !unmute.is_empty() {
            self.set_channels_muted(&unmute, user_id, false).await?;
        }
        Ok(())
    }

    /// `UpdateSidebarCategories` with its event and mutes.
    pub(crate) async fn update_sidebar_categories(
        &self,
        user_id: &str,
        team_id: &str,
        cats: &[SidebarCategory],
    ) -> ApiResult<Vec<SidebarCategory>> {
        let (updated, original) = self.update_categories(user_id, team_id, cats).await?;
        self.hub.publish(
            "sidebar_category_updated",
            json!({"updatedCategories": serde_json::to_string(&updated).unwrap()}),
            Broadcast { team_id: team_id.into(), user_id: user_id.into(), ..Default::default() },
        );
        self.mute_for_categories(user_id, &updated, &original).await?;
        Ok(updated)
    }

    /// `validateSidebarCategoryChannels`: only the user's channels of the team (and DMs), each once.
    async fn filter_category_channels(
        &self,
        user_id: &str,
        team_id: &str,
        cats: &mut [SidebarCategory],
    ) -> ApiResult<()> {
        let mine = crate::channels_api::channels_for_team_for_user_list(self, team_id, user_id, true, 0)
            .await
            .map_err(|e| {
                AppError::bad_request("api.invalid_channel", "Channel listed in the request doesn't belong to the user")
                    .detail(e.message)
            })?;
        let ids: std::collections::HashSet<&str> = mine.iter().map(|c| c.id.as_str()).collect();
        for c in cats {
            let mut seen = std::collections::HashSet::new();
            c.channel_ids.retain(|ch| ids.contains(ch.as_str()) && seen.insert(ch.clone()));
        }
        Ok(())
    }

    /// `SessionHasPermissionToCategory`.
    async fn category_perm(&self, s: &Session, user_id: &str, team_id: &str, category_id: &str) -> ApiResult<bool> {
        if self.has_perm(s, perm::EDIT_OTHER_USERS).await {
            return Ok(true);
        }
        Ok(self
            .sidebar_category(category_id)
            .await?
            .is_some_and(|c| c.user_id == s.user_id && c.user_id == user_id && c.team_id == team_id))
    }
}

/// The checks every category endpoint starts with: the user (or another with the right) and the team.
async fn check(app: &App, s: &Session, user_id: &str, team_id: &str) -> ApiResult<String> {
    let user_id = s.resolve(user_id);
    if !is_valid_id(&user_id) {
        return Err(AppError::invalid_url_param("user_id"));
    }
    if !is_valid_id(team_id) {
        return Err(AppError::invalid_url_param("team_id"));
    }
    app.require_user(s, &user_id).await?;
    app.require_team(s, team_id, perm::VIEW_TEAM).await?;
    Ok(user_id)
}

async fn check_category(app: &App, s: &Session, user_id: &str, team_id: &str, category_id: &str) -> ApiResult<String> {
    let user_id = s.resolve(user_id);
    if !is_valid_id(&user_id) {
        return Err(AppError::invalid_url_param("user_id"));
    }
    if !is_valid_id(team_id) {
        return Err(AppError::invalid_url_param("team_id"));
    }
    if category_id.is_empty() {
        return Err(AppError::invalid_url_param("category_id"));
    }
    if !app.category_perm(s, &user_id, team_id, category_id).await? {
        return Err(AppError::forbidden_perm(perm::EDIT_OTHER_USERS));
    }
    app.require_team(s, team_id, perm::VIEW_TEAM).await?;
    Ok(user_id)
}

async fn get_categories(
    St(app): St<State>,
    Auth(s): Auth,
    Path((user_id, team_id)): Path<(String, String)>,
) -> ApiResult<Json<Ordered>> {
    let user_id = check(&app, &s, &user_id, &team_id).await?;
    Ok(Json(app.sidebar_for_team_for_user(&user_id, &team_id).await?))
}

async fn create_category(
    St(app): St<State>,
    Auth(s): Auth,
    Path((user_id, team_id)): Path<(String, String)>,
    Body(c): Body<Option<SidebarCategory>>,
) -> ApiResult<Json<SidebarCategory>> {
    let user_id = check(&app, &s, &user_id, &team_id).await?;
    let mut c = c.ok_or_else(|| AppError::invalid_param("category"))?;
    if c.user_id != user_id || c.team_id != team_id {
        return Err(AppError::invalid_param("category"));
    }
    app.filter_category_channels(&user_id, &team_id, std::slice::from_mut(&mut c)).await?;
    let c = app.create_category(&user_id, &team_id, &c).await?;
    app.hub.publish(
        "sidebar_category_created",
        json!({"category_id": c.id}),
        Broadcast { team_id: team_id.clone(), user_id: user_id.clone(), ..Default::default() },
    );
    Ok(Json(c))
}

async fn get_order(
    St(app): St<State>,
    Auth(s): Auth,
    Path((user_id, team_id)): Path<(String, String)>,
) -> ApiResult<Json<Vec<String>>> {
    let user_id = check(&app, &s, &user_id, &team_id).await?;
    Ok(Json(app.category_order(&user_id, &team_id).await?))
}

async fn update_order(
    St(app): St<State>,
    Auth(s): Auth,
    Path((user_id, team_id)): Path<(String, String)>,
    Body(v): Body<Value>,
) -> ApiResult<Json<Vec<String>>> {
    let user_id = check(&app, &s, &user_id, &team_id).await?;
    let order: Vec<String> = serde_json::from_value(v)
        .map_err(|_| AppError::bad_request("api.payload.parse.error", "Error parsing the payload."))?;
    for id in &order {
        if !app.category_perm(&s, &user_id, &team_id, id).await? {
            return Err(AppError::invalid_param("category"));
        }
    }
    // UpdateSidebarCategoryOrder
    let existing = app.category_order(&user_id, &team_id).await?;
    if existing.len() != order.len() {
        return Err(categories_error(axum::http::StatusCode::INTERNAL_SERVER_ERROR));
    }
    if existing.iter().any(|id| !order.contains(id)) {
        return Err(categories_error(axum::http::StatusCode::BAD_REQUEST));
    }
    let mut tx = app.db.begin().await?;
    App::write_category_order(&mut tx, &order).await?;
    tx.commit().await?;
    app.hub.publish(
        "sidebar_category_order_updated",
        json!({"order": order}),
        Broadcast { team_id: team_id.clone(), user_id: user_id.clone(), ..Default::default() },
    );
    Ok(Json(order))
}

async fn get_category(
    St(app): St<State>,
    Auth(s): Auth,
    Path((user_id, team_id, id)): Path<(String, String, String)>,
) -> ApiResult<Json<SidebarCategory>> {
    check_category(&app, &s, &user_id, &team_id, &id).await?;
    app.sidebar_category(&id).await?.map(Json).ok_or_else(|| categories_error(axum::http::StatusCode::NOT_FOUND))
}

async fn update_categories(
    St(app): St<State>,
    Auth(s): Auth,
    Path((user_id, team_id)): Path<(String, String)>,
    Body(cats): Body<Vec<SidebarCategory>>,
) -> ApiResult<Json<Vec<SidebarCategory>>> {
    let user_id = check(&app, &s, &user_id, &team_id).await?;
    let mut cats = cats;
    for c in &cats {
        if !app.category_perm(&s, &user_id, &team_id, &c.id).await? {
            return Err(AppError::invalid_param("category"));
        }
    }
    app.filter_category_channels(&user_id, &team_id, &mut cats).await?;
    Ok(Json(app.update_sidebar_categories(&user_id, &team_id, &cats).await?))
}

async fn update_category(
    St(app): St<State>,
    Auth(s): Auth,
    Path((user_id, team_id, id)): Path<(String, String, String)>,
    Body(c): Body<Option<SidebarCategory>>,
) -> ApiResult<Json<SidebarCategory>> {
    let user_id = check_category(&app, &s, &user_id, &team_id, &id).await?;
    let mut c = c.ok_or_else(|| AppError::invalid_param("category"))?;
    if c.team_id != team_id || c.user_id != user_id {
        return Err(AppError::invalid_param("category"));
    }
    app.filter_category_channels(&user_id, &team_id, std::slice::from_mut(&mut c)).await?;
    c.id = id;
    let mut out = app.update_sidebar_categories(&user_id, &team_id, &[c]).await?;
    Ok(Json(out.remove(0)))
}

async fn delete_category(
    St(app): St<State>,
    Auth(s): Auth,
    Path((user_id, team_id, id)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let user_id = check_category(&app, &s, &user_id, &team_id, &id).await?;
    let c: Option<SidebarCategory> = crate::db::get(&app.db, &id).await?;
    let c = c.ok_or_else(|| categories_error(axum::http::StatusCode::INTERNAL_SERVER_ERROR))?;
    if c.r#type != category::CUSTOM {
        return Err(categories_error(axum::http::StatusCode::BAD_REQUEST));
    }
    let mut tx = app.db.begin().await?;
    sqlx::query("DELETE FROM SidebarCategories WHERE Id = $1").bind(&id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM SidebarChannels WHERE CategoryId = $1").bind(&id).execute(&mut *tx).await?;
    tx.commit().await?;
    app.hub.publish(
        "sidebar_category_deleted",
        json!({"category_id": id}),
        Broadcast { team_id, user_id, ..Default::default() },
    );
    Ok(Json(json!({"status": "OK"})))
}

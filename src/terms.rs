//! Custom terms of service: the latest terms (through their cache) and, licensed, new terms.

// Based on Mattermost server/channels/api4/terms_of_service.go, app/terms_of_service.go and
// store/localcachelayer/terms_of_service_layer.go, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State as St;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::app::{ApiResult, App, AppError, Auth, State};
use crate::perm::MANAGE_SYSTEM;

pub fn routes() -> Router<State> {
    Router::new().route("/api/v4/terms_of_service", get(latest).post(create))
}

fn no_rows() -> AppError {
    AppError::not_found("app.terms_of_service.get.no_rows.app_error", "No terms of service found.")
}

const SELECT: &str = "SELECT Id, COALESCE(CreateAt, 0), COALESCE(UserId, ''), COALESCE(Text, '') FROM TermsOfService";

fn to_json((id, create_at, user_id, text): (String, i64, String, String)) -> Arc<Value> {
    Arc::new(json!({"id": id, "create_at": create_at, "user_id": user_id, "text": text}))
}

impl App {
    /// `TermsOfServiceStore.GetLatest(true)`: cached under `latest` and the id.
    pub async fn latest_terms_of_service(&self) -> ApiResult<Arc<Value>> {
        if let Some(v) = self.caches.terms_of_service.get("latest") {
            return Ok(v);
        }
        let t: Option<(String, i64, String, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} ORDER BY CreateAt DESC LIMIT 1")))
                .fetch_optional(&self.db)
                .await
                .map_err(|e| AppError::internal("app.terms_of_service.get.app_error", e.to_string()))?;
        let v = to_json(t.ok_or_else(no_rows)?);
        let id = v["id"].as_str().unwrap_or_default().to_string();
        self.caches.terms_of_service.put(id, v.clone());
        self.caches.terms_of_service.put("latest".into(), v.clone());
        Ok(v)
    }

    /// `TermsOfServiceStore.Get(id, true)`.
    pub async fn terms_of_service(&self, id: &str) -> ApiResult<Arc<Value>> {
        if let Some(v) = self.caches.terms_of_service.get(id) {
            return Ok(v);
        }
        let t: Option<(String, i64, String, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE Id = $1")))
                .bind(id)
                .fetch_optional(&self.db)
                .await
                .map_err(|e| AppError::internal("app.terms_of_service.get.app_error", e.to_string()))?;
        let v = to_json(t.ok_or_else(no_rows)?);
        self.caches.terms_of_service.put(id.to_string(), v.clone());
        Ok(v)
    }
}

/// `getLatestTermsOfService`.
async fn latest(St(app): St<State>, _: Auth) -> ApiResult<Json<Value>> {
    Ok(Json((*app.latest_terms_of_service().await?).clone()))
}

/// `createTermsOfService`: custom terms of service need a license.
async fn create(St(app): St<State>, Auth(s): Auth, _body: Bytes) -> ApiResult<Json<Value>> {
    app.require(&s, MANAGE_SYSTEM).await?;
    Err(AppError::bad_request("api.create_terms_of_service.custom_terms_of_service_disabled.app_error", ""))
}

//! Sidebar categories (`/users/{user_id}/teams/{team_id}/channels/categories`).
//!
//! STUB: owned by the channels branch. Contract: [`App::create_initial_sidebar`].

use axum::Router;

use crate::app::{ApiResult, App, State};

pub fn routes() -> Router<State> {
    Router::new()
}

impl App {
    /// `createInitialSidebarCategories`: favorites, channels and direct messages for a user new to a team.
    pub async fn create_initial_sidebar(&self, _user_id: &str, _team_id: &str) -> ApiResult<()> {
        Ok(())
    }
}

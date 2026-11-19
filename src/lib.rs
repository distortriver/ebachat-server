pub mod api;
pub mod app;
pub mod cache;
pub mod channels;
pub mod channels_api;
pub mod commands;
pub mod commands_builtin;
pub mod config;
pub mod db;
pub mod drafts;
pub mod emoji;
pub mod file_api;
pub mod files;
pub mod hub;
pub mod i18n;
pub mod images;
pub mod integrations;
pub mod model;
pub mod notify;
pub mod opengraph;
pub mod password;
pub mod perm;
pub mod posts;
pub mod reminders;
pub mod preferences;
pub mod reactions;
pub mod search;
pub mod search_params;
pub mod sidebar;
pub mod status;
pub mod system;
pub mod teams;
pub mod threads;
pub mod user_agent;
pub mod user_edit;
pub mod user_image;
pub mod user_lists;
pub mod users;
pub mod webhooks;

/// What the API reports as its version: Mattermost's, whose API this is.
pub const SERVER_VERSION: &str = "12.1.0";
pub const SERVER_VERSION_FULL: &str = "12.1.0";
/// The 12.1.0 release's commit, as the client config reports it.
pub const BUILD_HASH: &str = "243e3d0135203ab9166fe8f2873b07f697b06fd4";

/// A Go `MM_SQLSETTINGS_DATASOURCE` DSN for sqlx: parameters it does not know are dropped.
pub fn db_url(dsn: &str) -> String {
    let Some((base, query)) = dsn.split_once('?') else { return dsn.to_string() };
    let kept: Vec<&str> = query.split('&').filter(|kv| kv.starts_with("sslmode=") || kv.starts_with("application_name=")).collect();
    if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }
}

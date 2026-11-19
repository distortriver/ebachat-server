pub mod api;
pub mod app;
pub mod cache;
pub mod channels;
pub mod config;
pub mod db;
pub mod emoji;
pub mod file_api;
pub mod files;
pub mod hub;
pub mod model;
pub mod password;
pub mod perm;
pub mod posts;
pub mod sidebar;
pub mod status;
pub mod users;

/// What the API reports as its version: Mattermost's, whose API this is.
pub const SERVER_VERSION: &str = "12.1.0";
pub const SERVER_VERSION_FULL: &str = "12.1.0";

/// A Go `MM_SQLSETTINGS_DATASOURCE` DSN for sqlx: parameters it does not know are dropped.
pub fn db_url(dsn: &str) -> String {
    let Some((base, query)) = dsn.split_once('?') else { return dsn.to_string() };
    let kept: Vec<&str> = query.split('&').filter(|kv| kv.starts_with("sslmode=") || kv.starts_with("application_name=")).collect();
    if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }
}

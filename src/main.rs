use std::time::Duration;

use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://mmuser:mmpass@localhost:5433/mattermost?sslmode=disable".into());
    let listen = std::env::var("LISTEN").unwrap_or_else(|_| "127.0.0.1:8066".into());
    let max: u32 = std::env::var("DB_MAX_CONNECTIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let db = PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&url)
        .await
        .expect("database");
    let app = ebachat_server::app::App::new(db);
    let listener = tokio::net::TcpListener::bind(&listen).await.expect("listen");
    eprintln!("ebachat-server {} on {listen}", ebachat_server::SERVER_VERSION);
    axum::serve(listener, ebachat_server::api::router(app)).await.unwrap();
}

use std::time::Duration;

use ebachat_server::{api, app::App, config::Config, db_url};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut cfg = Config::load().await?;
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| db_url(cfg.str("SqlSettings.DataSource")));
    let max: u32 = std::env::var("DB_MAX_CONNECTIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&url)
        .await?;
    cfg.diagnostic_id = sqlx::query_scalar("SELECT Value FROM Systems WHERE Name = 'DiagnosticId'")
        .fetch_optional(&db)
        .await?
        .unwrap_or_default();
    let listen = std::env::var("LISTEN").unwrap_or_else(|_| {
        let l = cfg.str("ServiceSettings.ListenAddress");
        if l.starts_with(':') { format!("0.0.0.0{l}") } else { l.to_string() }
    });
    let app = App::new(db, cfg)?;
    app.hub.set_app(&app);
    ebachat_server::status::spawn_jobs(app.clone());
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!("listening on {listen}");
    axum::serve(listener, api::router(app)).await?;
    Ok(())
}

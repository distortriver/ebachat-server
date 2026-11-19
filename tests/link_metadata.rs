//! Link metadata against the network and the stand's database: prints what `link_metadata_at` gives for the URLs
//! in `LINKS` (space separated), at `LINK_TS` (ms, default now), for comparing with the Go server's post embeds.
//!
//! `MM_CONFIG=... LINKS="https://..." cargo test --test link_metadata -- --ignored --nocapture`

use ebachat_server::app::{App, now_ms};
use ebachat_server::config::Config;

#[tokio::test]
#[ignore]
async fn print_link_metadata() {
    let cfg = Config::load().await.expect("MM_CONFIG");
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://mmuser:mmpass@localhost:5433/mattermost?sslmode=disable".into());
    let db = sqlx::PgPool::connect(&url).await.unwrap();
    let app = App::new(db, cfg).unwrap();
    let ts: i64 = std::env::var("LINK_TS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(now_ms);
    let old = std::env::var("LINK_OLD").is_ok();
    for link in std::env::var("LINKS").unwrap_or_default().split_whitespace() {
        let m = app.link_metadata_at(link, ts, !old).await;
        let out = m.map(|m| serde_json::json!({"opengraph": m.opengraph, "image": m.image}));
        println!("{link}\t{}", serde_json::to_string(&out).unwrap());
    }
}

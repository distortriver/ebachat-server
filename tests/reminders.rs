//! A due post reminder becomes the system bot's DM, as Go's `CheckPostReminders` sends it. Needs the dev stand
//! (`MM_CONFIG`, `DATABASE_URL`); run with `cargo test --test reminders -- --ignored`.

use ebachat_server::app::{App, now_ms};
use ebachat_server::config::Config;

#[tokio::test]
#[ignore]
async fn due_reminder_is_sent_by_the_system_bot() {
    let cfg = Config::load().await.unwrap();
    let url = std::env::var("DATABASE_URL").unwrap();
    let db = sqlx::PgPool::connect(&url).await.unwrap();
    let app = App::new(db.clone(), cfg).unwrap();
    app.hub.set_app(&app);

    let (post_id, user_id): (String, String) =
        sqlx::query_as("SELECT p.Id, u.Id FROM Posts p JOIN Users u ON u.Username = 'bench0002' WHERE p.DeleteAt = 0 AND p.Type = '' ORDER BY p.CreateAt DESC LIMIT 1")
            .fetch_one(&db)
            .await
            .unwrap();
    sqlx::query("INSERT INTO PostReminders (PostId, UserId, TargetTime) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(&post_id)
        .bind(&user_id)
        .bind(now_ms() / 1000 - 10)
        .execute(&db)
        .await
        .unwrap();
    let before = now_ms();
    app.check_post_reminders().await.unwrap();

    let bot = app.system_bot().await.unwrap();
    let (message, kind, props): (String, String, serde_json::Value) = sqlx::query_as(
        "SELECT Message, Type, Props FROM Posts WHERE UserId = $1 AND CreateAt >= $2 ORDER BY CreateAt DESC LIMIT 1",
    )
    .bind(&bot)
    .bind(before)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(kind, "reminder");
    assert!(message.contains(&format!("/pl/{post_id}")), "{message}");
    assert_eq!(props["post_id"], post_id);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM PostReminders WHERE PostId = $1 AND UserId = $2")
        .bind(&post_id)
        .bind(&user_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

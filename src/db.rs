//! Rows of Mattermost tables. `#[model(table = "...")]` implements [`Table`]; queries that are not plain
//! get/insert/update are written as SQL next to the code that needs them.

use sqlx::postgres::{PgArguments, PgRow};
use sqlx::{Decode, PgPool, Postgres, Row, Type};

pub type Query<'q> = sqlx::query::Query<'q, Postgres, PgArguments>;

pub trait Table: Sized + Send + Unpin + for<'r> sqlx::FromRow<'r, PgRow> {
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];
    /// `SELECT <columns> FROM <table>`; append `WHERE ...`.
    const SELECT: &'static str;
    const INSERT: &'static str;
    /// `UPDATE <table> SET <every column but id> WHERE id = $1`.
    const UPDATE: &'static str;
    /// Binds every column in [`Self::COLUMNS`] order.
    fn bind<'q>(&'q self, q: Query<'q>) -> Query<'q>;
}

/// A column that may be NULL, read as the zero value: Mattermost's Go structs have no NULLs.
pub fn col<'r, T>(row: &'r PgRow, name: &str) -> sqlx::Result<T>
where
    T: Decode<'r, Postgres> + Type<Postgres> + Default,
{
    Ok(row.try_get::<Option<T>, _>(name)?.unwrap_or_default())
}

pub async fn get<T: Table>(db: &PgPool, id: &str) -> sqlx::Result<Option<T>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{} WHERE id = $1", T::SELECT))).bind(id).fetch_optional(db).await
}

pub async fn insert<T: Table>(db: &PgPool, row: &T) -> sqlx::Result<()> {
    row.bind(sqlx::query(T::INSERT)).execute(db).await.map(drop)
}

pub async fn update<T: Table>(db: &PgPool, row: &T) -> sqlx::Result<u64> {
    Ok(row.bind(sqlx::query(T::UPDATE)).execute(db).await?.rows_affected())
}

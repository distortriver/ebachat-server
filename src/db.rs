//! Rows of Mattermost tables. `#[model(table = "...")]` implements [`Table`]; queries that are not plain
//! get/insert/update are written as SQL next to the code that needs them.

use sqlx::postgres::{PgArguments, PgRow};
use sqlx::{Decode, PgExecutor, Postgres, Row, Type};

pub type Query<'q> = sqlx::query::Query<'q, Postgres, PgArguments>;

pub trait Table: Sized + Send + Unpin + for<'r> sqlx::FromRow<'r, PgRow> {
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];
    /// The Postgres enum type of each column, "" for none.
    const PGTYPES: &'static [&'static str];
    /// `SELECT <columns> FROM <table>`; append `WHERE ...`.
    const SELECT: &'static str;
    const INSERT: &'static str;
    /// `UPDATE <table> SET <every column but id> WHERE id = $1`.
    const UPDATE: &'static str;
    /// Binds every column in [`Self::COLUMNS`] order.
    fn bind<'q>(&'q self, q: Query<'q>) -> Query<'q>;
}

/// The select list of `T` with a table alias, for joins: `c.id, c.type::text AS type, ...`.
pub fn cols<T: Table>(alias: &str) -> String {
    T::COLUMNS
        .iter()
        .zip(T::PGTYPES)
        .map(|(c, p)| if p.is_empty() { format!("{alias}.{c}") } else { format!("{alias}.{c}::text AS {c}") })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A column that may be NULL, read as the zero value: Mattermost's Go structs have no NULLs.
pub fn col<'r, T>(row: &'r PgRow, name: &str) -> sqlx::Result<T>
where
    T: Decode<'r, Postgres> + Type<Postgres> + Default,
{
    Ok(row.try_get::<Option<T>, _>(name)?.unwrap_or_default())
}

/// `SELECT ... WHERE <cond>` of `T`, one bind.
pub async fn one<'e, T: Table>(db: impl PgExecutor<'e>, cond: &str, arg: &str) -> sqlx::Result<Option<T>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{} WHERE {cond}", T::SELECT))).bind(arg).fetch_optional(db).await
}

pub async fn get<'e, T: Table>(db: impl PgExecutor<'e>, id: &str) -> sqlx::Result<Option<T>> {
    one(db, "id = $1", id).await
}

pub async fn insert<'e, T: Table>(db: impl PgExecutor<'e>, row: &T) -> sqlx::Result<()> {
    row.bind(sqlx::query(T::INSERT)).execute(db).await.map(drop)
}

pub async fn update<'e, T: Table>(db: impl PgExecutor<'e>, row: &T) -> sqlx::Result<u64> {
    Ok(row.bind(sqlx::query(T::UPDATE)).execute(db).await?.rows_affected())
}

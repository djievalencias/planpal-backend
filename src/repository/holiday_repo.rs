use crate::{error::AppError, model::holiday::HolidayConfig};
use chrono::NaiveDate;
use sqlx::PgPool;
use uuid::Uuid;

const SELECT_COLS: &str =
    "id, country, year, name, date, created_at, updated_at";

pub async fn list(
    pool: &PgPool,
    country: &str,
    year: i16,
) -> Result<Vec<HolidayConfig>, AppError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM holiday_configs
         WHERE country = $1 AND year = $2
         ORDER BY date ASC"
    ))
    .bind(country)
    .bind(year)
    .fetch_all(pool)
    .await?)
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<HolidayConfig>, AppError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM holiday_configs WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn create(
    pool: &PgPool,
    country: &str,
    year: i16,
    name: &str,
    date: NaiveDate,
) -> Result<HolidayConfig, AppError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO holiday_configs (country, year, name, date)
         VALUES ($1, $2, $3, $4)
         RETURNING {SELECT_COLS}"
    ))
    .bind(country)
    .bind(year)
    .bind(name)
    .bind(date)
    .fetch_one(pool)
    .await?)
}

pub async fn update(
    pool: &PgPool,
    id: Uuid,
    name: &str,
    date: NaiveDate,
) -> Result<Option<HolidayConfig>, AppError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE holiday_configs
         SET name = $1, date = $2, updated_at = NOW()
         WHERE id = $3
         RETURNING {SELECT_COLS}"
    ))
    .bind(name)
    .bind(date)
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let result =
        sqlx::query("DELETE FROM holiday_configs WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

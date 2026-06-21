pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod markdown;
pub mod models;
pub mod routes;
pub mod state;
pub mod views;

use std::sync::Arc;

use crate::config::Config;
use crate::error::AppError;
use crate::state::AppState;

/// Migrate the database, build the pool, and assemble shared state.
/// The router comes from `routes::router(state.clone())`.
pub fn build_state(config: Config) -> Result<AppState, AppError> {
    db::migrate(&config.db_path)?;
    let pool = db::create_pool(&config.db_path)?;
    Ok(AppState {
        pool,
        config: Arc::new(config),
    })
}

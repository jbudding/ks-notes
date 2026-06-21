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

/// Crate version, surfaced in the UI next to the brand.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fingerprint of the bundled static assets (set by `build.rs`), used as a
/// cache-busting `?v=` query on the `app.css` / `app.js` links so browsers pick
/// up changes despite the long `Cache-Control` on `/static/*`.
pub const ASSET_HASH: &str = env!("ASSET_HASH");

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

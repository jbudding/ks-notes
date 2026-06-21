use std::sync::Arc;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: deadpool_sqlite::Pool,
    pub config: Arc<Config>,
}

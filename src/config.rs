use clap::Parser;

/// Self-hosted memo/note server — a single-binary Memos clone.
#[derive(Parser, Debug, Clone)]
#[command(name = "ks-notes", version, about)]
pub struct Config {
    /// Address to bind to
    #[arg(long, env = "KSNOTES_BIND", default_value = "127.0.0.1")]
    pub bind: String,

    /// Port to listen on
    #[arg(long, env = "KSNOTES_PORT", default_value_t = 5230)]
    pub port: u16,

    /// Path to the SQLite database file (created if missing)
    #[arg(long, env = "KSNOTES_DB_PATH", default_value = "ks-notes.db")]
    pub db_path: std::path::PathBuf,

    /// Maximum upload size in MiB
    #[arg(long, env = "KSNOTES_MAX_UPLOAD_MB", default_value_t = 32)]
    pub max_upload_mb: usize,

    /// Set the Secure attribute on session cookies (enable when serving over HTTPS)
    #[arg(long, env = "KSNOTES_SECURE_COOKIES", default_value_t = false)]
    pub secure_cookies: bool,
}

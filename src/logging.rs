use std::sync::LazyLock;

use tracing_error::ErrorLayer;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::config;

pub static LOG_ENV: LazyLock<String> =
    LazyLock::new(|| format!("{}_LOG_LEVEL", config::PROJECT_NAME.clone()));
pub static LOG_FILE: LazyLock<String> = LazyLock::new(|| format!("{}.log", env!("CARGO_PKG_NAME")));

pub fn init() -> color_eyre::Result<()> {
    let directory = config::get_data_dir();
    std::fs::create_dir_all(directory.clone())?;
    let log_path = directory.join(LOG_FILE.clone());
    let log_file = std::fs::File::create(log_path)?;
    let env_filter = EnvFilter::builder().with_default_directive(tracing::Level::INFO.into());
    let env_filter = env_filter
        .try_from_env()
        .or_else(|_| env_filter.with_env_var(LOG_ENV.clone()).from_env())?;

    let file_layer = fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_writer(log_file)
        .with_target(false)
        .with_ansi(false)
        .with_filter(env_filter);

    // stderr 输出 WARN 及以上（方便调试）
    let stderr_layer = fmt::layer()
        .with_target(false)
        .with_level(true)
        .with_filter(tracing::level_filters::LevelFilter::INFO);

    tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .with(ErrorLayer::default())
        .try_init()?;
    Ok(())
}

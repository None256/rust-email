use clap::Parser;
use cli::Cli;

use crate::app::App;

mod action;
mod app;
mod cli;
mod components;
mod config;
mod database;
mod errors;
mod logging;
mod tui;
mod utils;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    // 自动加载 .env 文件（如果存在）
    dotenvy::dotenv().ok();

    crate::errors::init()?;
    crate::logging::init()?;

    let args = Cli::parse();
    let database = database::connect(&config::get_data_dir().join("rust-email.db")).await?;
    let mut app = App::new(args.tick_rate, args.frame_rate, database).await?;
    app.run().await?;
    Ok(())

}

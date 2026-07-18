use std::path::Path;

use aes_gcm::{Aes256Gcm, Nonce, aead::{Aead, KeyInit}};
use argon2::Argon2;
use clap::Parser;
use cli::Cli;
use rand::RngCore;

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

fn prompt_master_password(data_dir: &Path) -> color_eyre::Result<[u8; 32]> {
    let salt_path = data_dir.join("salt");
    let check_path = data_dir.join("keycheck");

    if salt_path.exists() {
        let salt: [u8; 16] = std::fs::read(&salt_path)?
            .try_into()
            .map_err(|_| color_eyre::eyre::eyre!("invalid salt file"))?;
        let check_data = std::fs::read(&check_path)?;

        loop {
            eprint!("请输入主密码: ");
            let password = rpassword::read_password()?;
            let key = derive_master_key(&password, &salt);

            if verify_master_key(&key, &check_data) {
                eprintln!();
                return Ok(key);
            }
            eprintln!("密码错误，请重试（忘记密码？删除 .data/ 目录即可重置，但旧账户数据会丢失）\n");
        }
    } else {
        eprintln!("首次使用，请设置主密码（用于加密存储邮箱密码）");
        eprintln!("⚠ 警告：此密码无法找回！忘记后将永久丢失所有已存储的邮箱账户。");
        eprintln!();

        eprint!("请输入主密码: ");
        let password = rpassword::read_password()?;
        eprint!("请再次输入确认: ");
        let confirm = rpassword::read_password()?;

        if password != confirm {
            return Err(color_eyre::eyre::eyre!("两次输入的密码不一致"));
        }
        if password.len() < 4 {
            return Err(color_eyre::eyre::eyre!("密码至少需要 4 个字符"));
        }

        let mut salt = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut salt);
        let key = derive_master_key(&password, &salt);
        let check_data = make_master_check(&key);

        std::fs::write(&salt_path, salt)?;
        std::fs::write(&check_path, check_data)?;
        eprintln!();
        Ok(key)
    }
}

fn derive_master_key(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("argon2 key derivation failed");
    key
}

fn make_master_check(key: &[u8; 32]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("invalid key length");
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let magic = b"RUST_EMAIL_MASTER_OK";
    let mut buf = nonce.to_vec();
    buf.extend(cipher.encrypt(Nonce::from_slice(&nonce), magic.as_ref()).expect("encrypt check failed"));
    buf
}

fn verify_master_key(key: &[u8; 32], check_data: &[u8]) -> bool {
    if check_data.len() < 12 {
        return false;
    }
    let cipher = Aes256Gcm::new_from_slice(key).expect("invalid key length");
    cipher
        .decrypt(Nonce::from_slice(&check_data[..12]), &check_data[12..])
        .map(|p| p == b"RUST_EMAIL_MASTER_OK")
        .unwrap_or(false)
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    dotenvy::dotenv().ok();

    crate::errors::init()?;
    crate::logging::init()?;

    let data_dir = config::get_data_dir();
    std::fs::create_dir_all(&data_dir)?;

    let master_key = prompt_master_password(&data_dir)?;
    database::set_password_key(master_key);

    let args = Cli::parse();
    let database = database::connect(&data_dir.join("rust-email.db")).await?;
    let mut app = App::new(args.tick_rate, args.frame_rate, database).await?;
    app.run().await?;
    Ok(())
}

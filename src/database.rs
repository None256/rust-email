use std::path::Path;
use std::sync::OnceLock;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use rand::RngCore;
use tracing::{info, warn};

use mail_protocol::{
    AccountConfig, AttachmentMeta, Email, EmailSummary, Folder, MailFlag, SecurityMode,
};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

static PASSWORD_KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn parent_path(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new("."))
}

fn set_restrictive_permissions(path: &Path) -> color_eyre::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

fn init_password_key(dir: &Path) -> color_eyre::Result<()> {
    if PASSWORD_KEY.get().is_some() {
        return Ok(());
    }
    let key_path = dir.join("password.key");
    let mut key = [0u8; 32];
    if key_path.exists() {
        let bytes = std::fs::read(&key_path)?;
        key.copy_from_slice(
            bytes
                .get(..32)
                .ok_or_else(|| color_eyre::eyre::eyre!("invalid password key"))?,
        );
        info!(
            "已加载密码密钥: {} （请勿删除此文件，否则所有已存储的账户密码将无法解密）",
            key_path.display()
        );
    } else {
        rand::thread_rng().fill_bytes(&mut key);
        std::fs::write(&key_path, key)?;
        set_restrictive_permissions(&key_path)?;
    }
    let _ = PASSWORD_KEY.set(key);
    Ok(())
}

fn encrypt_password(password: &str) -> color_eyre::Result<String> {
    let key = PASSWORD_KEY.get_or_init(|| {
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        key
    });
    let cipher = Aes256Gcm::new_from_slice(key).unwrap();
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), password.as_bytes())
        .map_err(|_| color_eyre::eyre::eyre!("password encryption failed"))?;
    Ok(format!(
        "v1:{}:{}",
        B64.encode(nonce),
        B64.encode(ciphertext)
    ))
}

fn decrypt_password(value: &str) -> color_eyre::Result<String> {
    let key = PASSWORD_KEY
        .get()
        .ok_or_else(|| color_eyre::eyre::eyre!("password key is not initialized"))?;
    let mut parts = value.split(':');
    if parts.next() != Some("v1") {
        return Err(color_eyre::eyre::eyre!("unsupported password format"));
    }
    let nonce = B64.decode(
        parts
            .next()
            .ok_or_else(|| color_eyre::eyre::eyre!("invalid password"))?,
    )?;
    if nonce.len() != 12 {
        return Err(color_eyre::eyre::eyre!("invalid password nonce"));
    }
    let ciphertext = B64.decode(
        parts
            .next()
            .ok_or_else(|| color_eyre::eyre::eyre!("invalid password"))?,
    )?;
    let cipher = Aes256Gcm::new_from_slice(key).unwrap();
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| color_eyre::eyre::eyre!("password decryption failed"))?;
    Ok(String::from_utf8(plaintext)?)
}

async fn migrate_plaintext_passwords(pool: &SqlitePool) -> color_eyre::Result<()> {
    let rows = sqlx::query("SELECT id,password FROM accounts WHERE (password_encrypted IS NULL OR password_encrypted='') AND password <> ''")
        .fetch_all(pool).await?;
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let password: String = row.try_get("password")?;
        sqlx::query("UPDATE accounts SET password='', password_encrypted=? WHERE id=?")
            .bind(encrypt_password(&password)?)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub display_name: Option<String>,
    pub config: AccountConfig,
}

#[derive(Debug, Clone)]
pub struct NewAccount {
    pub email: String,
    pub display_name: Option<String>,
    pub config: AccountConfig,
}

pub async fn connect(path: &Path) -> color_eyre::Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    init_password_key(parent_path(path))?;
    migrate_plaintext_passwords(&pool).await?;
    Ok(pool)
}

pub async fn list_accounts(pool: &SqlitePool) -> color_eyre::Result<Vec<Account>> {
    let rows = sqlx::query("SELECT id,email,display_name,username,password,password_encrypted,imap_host,imap_port,smtp_host,smtp_port,security_mode FROM accounts ORDER BY id")
        .fetch_all(pool).await?;
    rows.iter().map(account_from_row).collect()
}

pub async fn get_account(pool: &SqlitePool, id: i64) -> color_eyre::Result<Option<Account>> {
    let row = sqlx::query("SELECT id,email,display_name,username,password,password_encrypted,imap_host,imap_port,smtp_host,smtp_port,security_mode FROM accounts WHERE id=?")
        .bind(id).fetch_optional(pool).await?;
    row.as_ref().map(account_from_row).transpose()
}

/// Inserts an account, or updates it when the email address already exists.
pub async fn save_account(pool: &SqlitePool, account: &NewAccount) -> color_eyre::Result<Account> {
    let encrypted = encrypt_password(&account.config.password)?;
    let id: i64 = sqlx::query_scalar("INSERT INTO accounts (email,display_name,username,password,password_encrypted,imap_host,imap_port,smtp_host,smtp_port,security_mode) VALUES (?,?,?,?,?,?,?,?,?,?) ON CONFLICT(email) DO UPDATE SET display_name=excluded.display_name,username=excluded.username,password='',password_encrypted=excluded.password_encrypted,imap_host=excluded.imap_host,imap_port=excluded.imap_port,smtp_host=excluded.smtp_host,smtp_port=excluded.smtp_port,security_mode=excluded.security_mode,updated_at=unixepoch() RETURNING id")
        .bind(&account.email).bind(&account.display_name).bind(&account.config.username)
        .bind("").bind(encrypted).bind(&account.config.imap_host).bind(i64::from(account.config.imap_port))
        .bind(&account.config.smtp_host).bind(i64::from(account.config.smtp_port)).bind(security_to_str(&account.config.security))
        .fetch_one(pool).await?;
    get_account(pool, id)
        .await?
        .ok_or_else(|| color_eyre::eyre::eyre!("saved account {id} not found"))
}

pub async fn delete_account(pool: &SqlitePool, id: i64) -> color_eyre::Result<bool> {
    Ok(sqlx::query("DELETE FROM accounts WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

pub async fn cache_folders(
    pool: &SqlitePool,
    account_id: i64,
    folders: &[Folder],
) -> color_eyre::Result<()> {
    let mut tx = pool.begin().await?;
    for folder in folders {
        sqlx::query("INSERT INTO folders (account_id,name,delimiter,attributes,last_synced_at) VALUES (?,?,?,?,unixepoch()) ON CONFLICT(account_id,name) DO UPDATE SET delimiter=excluded.delimiter,attributes=excluded.attributes,last_synced_at=unixepoch()")
            .bind(account_id).bind(&folder.name).bind(&folder.delimiter)
            .bind(serde_json::to_string(&folder.attributes)?).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn cached_folders(pool: &SqlitePool, account_id: i64) -> color_eyre::Result<Vec<Folder>> {
    sqlx::query("SELECT name,delimiter,attributes FROM folders WHERE account_id=? ORDER BY id")
        .bind(account_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| {
            Ok(Folder {
                name: r.try_get("name")?,
                delimiter: r.try_get("delimiter")?,
                attributes: serde_json::from_str(&r.try_get::<String, _>("attributes")?)?,
            })
        })
        .collect()
}

pub async fn cache_email_summaries(
    pool: &SqlitePool,
    account_id: i64,
    folder: &str,
    mails: &[EmailSummary],
) -> color_eyre::Result<()> {
    let folder_id = ensure_folder(pool, account_id, folder).await?;
    let mut tx = pool.begin().await?;
    for mail in mails {
        sqlx::query("INSERT INTO messages (folder_id,uid,message_id,sender,recipients,cc,subject,sent_at,size,flags,has_attachments,cached_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,unixepoch()) ON CONFLICT(folder_id,uid) DO UPDATE SET message_id=excluded.message_id,sender=excluded.sender,recipients=excluded.recipients,cc=excluded.cc,subject=excluded.subject,sent_at=excluded.sent_at,size=excluded.size,flags=excluded.flags,has_attachments=excluded.has_attachments,cached_at=unixepoch()")
            .bind(folder_id).bind(i64::from(mail.uid)).bind(&mail.message_id).bind(&mail.from)
            .bind(serde_json::to_string(&split_addresses(&mail.to))?)
            .bind(serde_json::to_string(&mail.cc.as_deref().map(split_addresses).unwrap_or_default())?)
            .bind(&mail.subject).bind(&mail.date).bind(i64::try_from(mail.size).unwrap_or(i64::MAX))
            .bind(serde_json::to_string(&mail.flags)?).bind(mail.has_attachments).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn cached_email_summaries(
    pool: &SqlitePool,
    account_id: i64,
    folder: &str,
    limit: u32,
) -> color_eyre::Result<Vec<EmailSummary>> {
    sqlx::query("SELECT m.uid,m.message_id,m.sender,m.recipients,m.cc,m.subject,m.sent_at,m.size,m.flags,m.has_attachments FROM messages m JOIN folders f ON f.id=m.folder_id WHERE f.account_id=? AND f.name=? ORDER BY m.uid DESC LIMIT ?")
        .bind(account_id).bind(folder).bind(i64::from(limit)).fetch_all(pool).await?.into_iter().map(summary_from_row).collect()
}

pub async fn cache_email(
    pool: &SqlitePool,
    account_id: i64,
    email: &Email,
) -> color_eyre::Result<()> {
    let folder_id = ensure_folder(pool, account_id, &email.folder).await?;
    let mut tx = pool.begin().await?;
    let message_id: i64 = sqlx::query_scalar("INSERT INTO messages (folder_id,uid,message_id,sender,recipients,cc,bcc,reply_to,subject,sent_at,flags,has_attachments,body_text,body_html,in_reply_to,references_json,cached_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,unixepoch()) ON CONFLICT(folder_id,uid) DO UPDATE SET message_id=excluded.message_id,sender=excluded.sender,recipients=excluded.recipients,cc=excluded.cc,bcc=excluded.bcc,reply_to=excluded.reply_to,subject=excluded.subject,sent_at=excluded.sent_at,flags=excluded.flags,has_attachments=excluded.has_attachments,body_text=excluded.body_text,body_html=excluded.body_html,in_reply_to=excluded.in_reply_to,references_json=excluded.references_json,cached_at=unixepoch() RETURNING id")
        .bind(folder_id).bind(i64::from(email.uid)).bind(&email.message_id).bind(&email.from)
        .bind(serde_json::to_string(&email.to)?).bind(serde_json::to_string(&email.cc)?).bind(serde_json::to_string(&email.bcc)?)
        .bind(&email.reply_to).bind(&email.subject).bind(&email.date).bind(serde_json::to_string(&email.flags)?)
        .bind(!email.attachments.is_empty()).bind(&email.body_text).bind(&email.body_html).bind(&email.in_reply_to)
        .bind(serde_json::to_string(&email.references)?).fetch_one(&mut *tx).await?;
    sqlx::query("DELETE FROM attachments WHERE message_id=?")
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
    for a in &email.attachments {
        sqlx::query("INSERT INTO attachments (message_id,filename,mime_type,size,content_id,part_id,transfer_encoding) VALUES (?,?,?,?,?,?,?)")
            .bind(message_id).bind(&a.filename).bind(&a.mime_type).bind(i64::try_from(a.size).unwrap_or(i64::MAX))
            .bind(&a.content_id).bind(&a.part_id).bind(&a.transfer_encoding).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn cached_email(
    pool: &SqlitePool,
    account_id: i64,
    folder: &str,
    uid: u32,
) -> color_eyre::Result<Option<Email>> {
    let row = sqlx::query("SELECT m.id,m.uid,m.message_id,m.sender,m.recipients,m.cc,m.bcc,m.reply_to,m.subject,m.sent_at,m.flags,m.body_text,m.body_html,m.in_reply_to,m.references_json FROM messages m JOIN folders f ON f.id=m.folder_id WHERE f.account_id=? AND f.name=? AND m.uid=?")
        .bind(account_id).bind(folder).bind(i64::from(uid)).fetch_optional(pool).await?;
    let Some(r) = row else { return Ok(None) };
    let attachments = sqlx::query("SELECT filename,mime_type,size,content_id,part_id,transfer_encoding FROM attachments WHERE message_id=? ORDER BY id")
        .bind(r.try_get::<i64,_>("id")?).fetch_all(pool).await?.into_iter().map(|a| Ok(AttachmentMeta {
            filename: a.try_get("filename")?, mime_type: a.try_get("mime_type")?,
            size: u64::try_from(a.try_get::<i64,_>("size")?).unwrap_or_default(), content_id: a.try_get("content_id")?, part_id: a.try_get("part_id")?,
            transfer_encoding: a.try_get("transfer_encoding")?,
        })).collect::<color_eyre::Result<Vec<_>>>()?;
    Ok(Some(Email {
        uid: r.try_get::<i64, _>("uid")? as u32,
        folder: folder.into(),
        message_id: r.try_get("message_id")?,
        from: r.try_get("sender")?,
        to: json_column(&r, "recipients")?,
        cc: json_column(&r, "cc")?,
        bcc: json_column(&r, "bcc")?,
        reply_to: r.try_get("reply_to")?,
        date: r
            .try_get::<Option<String>, _>("sent_at")?
            .unwrap_or_default(),
        subject: r.try_get("subject")?,
        body_text: r.try_get("body_text")?,
        body_html: r.try_get("body_html")?,
        attachments,
        in_reply_to: r.try_get("in_reply_to")?,
        references: json_column(&r, "references_json")?,
        flags: json_column(&r, "flags")?,
    }))
}

async fn ensure_folder(pool: &SqlitePool, account_id: i64, name: &str) -> color_eyre::Result<i64> {
    Ok(sqlx::query_scalar("INSERT INTO folders (account_id,name) VALUES (?,?) ON CONFLICT(account_id,name) DO UPDATE SET name=excluded.name RETURNING id")
        .bind(account_id).bind(name).fetch_one(pool).await?)
}

fn account_from_row(r: &sqlx::sqlite::SqliteRow) -> color_eyre::Result<Account> {
    let encrypted: Option<String> = r.try_get("password_encrypted")?;
    let password = match encrypted.filter(|v| !v.is_empty()) {
        Some(v) => decrypt_password(&v)?,
        None => r.try_get("password")?,
    };
    Ok(Account {
        id: r.try_get("id")?,
        email: r.try_get("email")?,
        display_name: r.try_get("display_name")?,
        config: AccountConfig {
            username: r.try_get("username")?,
            password,
            imap_host: r.try_get("imap_host")?,
            imap_port: r.try_get::<i64, _>("imap_port")? as u16,
            smtp_host: r.try_get("smtp_host")?,
            smtp_port: r.try_get::<i64, _>("smtp_port")? as u16,
            security: security_from_str(&r.try_get::<String, _>("security_mode")?),
        },
    })
}

fn summary_from_row(r: sqlx::sqlite::SqliteRow) -> color_eyre::Result<EmailSummary> {
    let to: Vec<String> = json_column(&r, "recipients")?;
    let cc: Vec<String> = json_column(&r, "cc")?;
    Ok(EmailSummary {
        uid: r.try_get::<i64, _>("uid")? as u32,
        message_id: r.try_get("message_id")?,
        from: r.try_get("sender")?,
        to: to.join(", "),
        cc: (!cc.is_empty()).then(|| cc.join(", ")),
        subject: r.try_get("subject")?,
        date: r
            .try_get::<Option<String>, _>("sent_at")?
            .unwrap_or_default(),
        size: u64::try_from(r.try_get::<i64, _>("size")?).unwrap_or_default(),
        flags: json_column::<Vec<MailFlag>>(&r, "flags")?,
        has_attachments: r.try_get("has_attachments")?,
    })
}

fn json_column<T: serde::de::DeserializeOwned>(
    r: &sqlx::sqlite::SqliteRow,
    name: &str,
) -> color_eyre::Result<T> {
    Ok(serde_json::from_str(&r.try_get::<String, _>(name)?)?)
}
fn split_addresses(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}
fn security_to_str(s: &SecurityMode) -> &'static str {
    match s {
        SecurityMode::Tls => "tls",
        SecurityMode::StartTls => "start_tls",
        SecurityMode::None => "none",
    }
}
fn security_from_str(s: &str) -> SecurityMode {
    match s {
        "start_tls" => SecurityMode::StartTls,
        "none" => SecurityMode::None,
        _ => SecurityMode::Tls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn pool() -> SqlitePool {
        let p = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(":memory:")
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&p).await.unwrap();
        p
    }
    fn account() -> NewAccount {
        NewAccount {
            email: "u@example.com".into(),
            display_name: Some("U".into()),
            config: AccountConfig {
                imap_host: "imap.example.com".into(),
                imap_port: 993,
                smtp_host: "smtp.example.com".into(),
                smtp_port: 465,
                username: "u@example.com".into(),
                password: "secret".into(),
                security: SecurityMode::Tls,
            },
        }
    }
    #[tokio::test]
    async fn account_crud() {
        let p = pool().await;
        let a = save_account(&p, &account()).await.unwrap();
        let stored = sqlx::query("SELECT password,password_encrypted FROM accounts WHERE id=?")
            .bind(a.id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(stored.get::<String, _>("password"), "");
        let encrypted = stored.get::<String, _>("password_encrypted");
        assert!(encrypted.starts_with("v1:"));
        assert!(!encrypted.contains("secret"));
        assert_eq!(list_accounts(&p).await.unwrap().len(), 1);
        let mut n = account();
        n.config.password = "new".into();
        assert_eq!(save_account(&p, &n).await.unwrap().id, a.id);
        assert_eq!(
            get_account(&p, a.id)
                .await
                .unwrap()
                .unwrap()
                .config
                .password,
            "new"
        );
        assert!(delete_account(&p, a.id).await.unwrap());
    }
    #[tokio::test]
    async fn email_cache() {
        let p = pool().await;
        let a = save_account(&p, &account()).await.unwrap();
        let s = EmailSummary {
            uid: 1,
            message_id: None,
            from: "a".into(),
            to: "b".into(),
            cc: None,
            subject: "s".into(),
            date: "d".into(),
            size: 1,
            flags: vec![MailFlag::Seen],
            has_attachments: false,
        };
        cache_email_summaries(&p, a.id, "INBOX", &[s])
            .await
            .unwrap();
        assert_eq!(
            cached_email_summaries(&p, a.id, "INBOX", 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

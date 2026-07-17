use std::pin::Pin;

use async_imap::{
    Client as ImapClientInner, Session as ImapSessionInner,
    types::{Fetch, Flag, Name},
};
use async_native_tls::{TlsConnector, TlsStream};
use base64::Engine;
use futures::{Stream, StreamExt};
use imap_proto::types::{Address, SectionPath};
use mailparse::{ParsedMail, parse_content_disposition, parse_mail};
use tokio::net::TcpStream;
use tracing::{debug, error, info, warn};
use tokio::sync::Mutex;
use tokio_util::compat::TokioAsyncReadCompatExt;

use crate::backend::*;
use crate::error::MailError;
use crate::smtp::SmtpSender;

// ── 类型别名 ────────────────────────────────────────────────────────

type CompatTlsStream = TlsStream<tokio_util::compat::Compat<TcpStream>>;
type ImapSession = ImapSessionInner<CompatTlsStream>;

// ── MailClient ──────────────────────────────────────────────────────

pub struct MailClient {
    inner: Mutex<ClientInner>,
}

struct ClientInner {
    config: Option<AccountConfig>,
    session: Option<ImapSession>,
}

impl MailClient {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ClientInner {
                config: None,
                session: None,
            }),
        }
    }
}

impl Default for MailClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl MailBackend for MailClient {
    // ── 连接生命周期 ────────────────────────────────────────────────

    async fn connect(&mut self, config: &AccountConfig) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;

        if let Some(mut session) = inner.session.take() {
            info!("断开旧会话");
            let _ = session.logout().await;
        }

        info!(
            "连接 {}:{} (security={:?})",
            config.imap_host, config.imap_port, config.security
        );

        match config.security {
            SecurityMode::Tls => {
                info!("TCP 连接 {}:{}...", config.imap_host, config.imap_port);
                let tcp = TcpStream::connect((config.imap_host.as_str(), config.imap_port))
                    .await
                    .map_err(|e| {
                        error!("TCP 连接失败: {e}");
                        MailError::Connection(format!("TCP connect: {e}"))
                    })?;
                info!("TCP 已连接");

                let tls = TlsConnector::new();
                let compat = tcp.compat();
                let tls_stream = tls
                    .connect(&config.imap_host, compat)
                    .await
                    .map_err(|e| {
                        error!("TLS 握手失败: {e}");
                        MailError::Tls(format!("TLS handshake: {e}"))
                    })?;
                info!("TLS 握手成功");

                let mut client = ImapClientInner::new(tls_stream);
                info!("发送 IMAP ID...");
                client
                    .run_command_and_check_ok(
                        r#"ID ("name" "rust-email" "version" "0.1")"#,
                        None,
                    )
                    .await
                    .map_err(|e| {
                        warn!("ID 命令失败: {e}");
                        MailError::Protocol(format!("ID: {e}"))
                    })?;
                info!("IMAP 登录中...");
                let mut session = client
                    .login(&config.username, &config.password)
                    .await
                    .map_err(|(e, _)| {
                        error!("IMAP 登录失败: {e}");
                        MailError::Authentication(format!("login failed: {e}"))
                    })?;
                info!("IMAP 登录成功");

                match session.select("INBOX").await {
                    Ok(_) => info!("连接后 SELECT INBOX 成功"),
                    Err(e) => warn!("连接后 SELECT INBOX 失败: {e}"),
                }
                inner.config = Some(config.clone());
                inner.session = Some(session);
                Ok(())
            }
            SecurityMode::StartTls => {
                let tcp = TcpStream::connect((config.imap_host.as_str(), config.imap_port))
                    .await
                    .map_err(|e| MailError::Connection(format!("TCP connect: {e}")))?;

                let mut client = ImapClientInner::new(tcp.compat());
                client
                    .run_command_and_check_ok("STARTTLS", None)
                    .await
                    .map_err(|e| MailError::Tls(format!("STARTTLS: {e}")))?;

                let stream = client.into_inner();
                let tls = TlsConnector::new();
                let tls_stream = tls
                    .connect(&config.imap_host, stream)
                    .await
                    .map_err(|e| MailError::Tls(format!("TLS handshake: {e}")))?;

                let mut client = ImapClientInner::new(tls_stream);
                info!("发送 IMAP ID...");
                client
                    .run_command_and_check_ok(
                        r#"ID ("name" "rust-email" "version" "0.1")"#,
                        None,
                    )
                    .await
                    .map_err(|e| {
                        warn!("ID 命令失败: {e}");
                        MailError::Protocol(format!("ID: {e}"))
                    })?;
                info!("IMAP 登录中...");
                let mut session = client
                    .login(&config.username, &config.password)
                    .await
                    .map_err(|(e, _)| MailError::Authentication(format!("login failed: {e}")))?;

                // 连接后验证：尝试 SELECT INBOX 确认会话可用
                let _ = session.select("INBOX").await;
                inner.config = Some(config.clone());
                inner.session = Some(session);
                Ok(())
            }
            SecurityMode::None => Err(MailError::Connection(
                "unencrypted IMAP not supported".into(),
            )),
        }
    }

    async fn disconnect(&mut self) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        if let Some(mut session) = inner.session.take() {
            session
                .logout()
                .await
                .map_err(|e| MailError::Protocol(format!("logout: {e}")))?;
        }
        inner.config = None;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        match self.inner.try_lock() {
            Ok(inner) => inner.session.is_some(),
            Err(_) => false,
        }
    }

    // ── 文件夹操作 ──────────────────────────────────────────────────

    async fn list_folders(&self) -> Result<Vec<Folder>, MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;

        info!("获取文件夹列表...");
        let folders = {
            let mut stream = session
                .list(None, Some("*"))
                .await
                .map_err(|e| {
                    error!("LIST 命令失败: {e}");
                    MailError::Protocol(format!("LIST: {e}"))
                })?;

            let mut folders = Vec::new();
            while let Some(item) = stream.next().await {
                let name: Name = item.map_err(|e| MailError::Protocol(format!("LIST stream: {e}")))?;
                folders.push(Folder {
                    name: name.name().to_string(),
                    delimiter: name.delimiter().unwrap_or("").to_string(),
                    attributes: name.attributes().iter().map(name_attr_to_string).collect(),
                });
            }
            folders
        };

        info!(
            "获取到 {} 个文件夹: {:?}",
            folders.len(),
            folders.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        Ok(folders)
    }

    async fn create_folder(&self, name: &str) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;
        session
            .create(name)
            .await
            .map_err(|e| MailError::Protocol(format!("CREATE: {e}")))?;
        Ok(())
    }

    async fn delete_folder(&self, name: &str) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;
        session
            .delete(name)
            .await
            .map_err(|e| MailError::Protocol(format!("DELETE: {e}")))?;
        Ok(())
    }

    async fn rename_folder(&self, old_name: &str, new_name: &str) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;
        session
            .rename(old_name, new_name)
            .await
            .map_err(|e| MailError::Protocol(format!("RENAME: {e}")))?;
        Ok(())
    }

    // ── 邮件列表 ────────────────────────────────────────────────────

    async fn fetch_latest_messages(
        &self,
        folder: &str,
        count: u32,
    ) -> Result<Vec<EmailSummary>, MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;

        info!("选择文件夹 '{folder}'...");
        let mailbox = select_mailbox(session, folder).await?;
        let total = mailbox.exists;
        info!("文件夹 '{folder}' 共 {total} 封邮件");
        if total == 0 || count == 0 {
            return Ok(Vec::new());
        }

        let count = count.min(total);
        let start = total.saturating_sub(count) + 1;
        let sequence = format!("{start}:*");
        debug!("FETCH 序列: {sequence}");

        let summaries = fetch_summaries_by_seq(session, &sequence).await?;
        info!("获取到 {} 封邮件摘要", summaries.len());
        Ok(summaries)
    }

    async fn fetch_messages_before(
        &self,
        folder: &str,
        before_uid: u32,
        count: u32,
    ) -> Result<Vec<EmailSummary>, MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;

        select_mailbox(session, folder).await?;

        let start_uid = before_uid.saturating_sub(count * 2).max(1);
        let sequence = format!("{start_uid}:{before_uid}");

        let mut summaries = fetch_summaries_by_uid(session, &sequence).await?;
        summaries.retain(|s| s.uid < before_uid);
        summaries.sort_by(|a, b| b.uid.cmp(&a.uid));
        summaries.truncate(count as usize);

        Ok(summaries)
    }

    async fn fetch_new_messages(
        &self,
        folder: &str,
        since_uid: u32,
    ) -> Result<Vec<EmailSummary>, MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;

        select_mailbox(session, folder).await?;

        let sequence = format!("{}:*", since_uid + 1);
        fetch_summaries_by_uid(session, &sequence).await
    }

    // ── 邮件内容 ────────────────────────────────────────────────────

    async fn fetch_message(&self, folder: &str, uid: u32) -> Result<Email, MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;

        select_mailbox(session, folder).await?;

        let sequence = format!("{uid}");
        let mut stream = session
            .uid_fetch(&sequence, "(UID FLAGS BODY[])")
            .await
            .map_err(|e| MailError::Protocol(format!("UID FETCH: {e}")))?;

        let fetch = stream
            .next()
            .await
            .ok_or(MailError::MessageNotFound(format!(
                "message {uid} not found in {folder}"
            )))?
            .map_err(|e| MailError::Protocol(format!("fetch: {e}")))?;

        let raw = fetch
            .body()
            .ok_or(MailError::Parse("empty message body".into()))?;

        let flags: Vec<MailFlag> = fetch.flags().map(parse_flag).collect();

        let parsed = parse_mail(raw).map_err(|e| MailError::Parse(format!("MIME parse: {e}")))?;

        Ok(Email {
            uid: fetch.uid.unwrap_or(uid),
            folder: folder.to_string(),
            message_id: get_header(&parsed, "Message-ID"),
            from: get_header(&parsed, "From").unwrap_or_default(),
            to: split_addr_list(&get_header(&parsed, "To").unwrap_or_default()),
            cc: split_addr_list(&get_header(&parsed, "Cc").unwrap_or_default()),
            bcc: split_addr_list(&get_header(&parsed, "Bcc").unwrap_or_default()),
            reply_to: get_header(&parsed, "Reply-To"),
            date: get_header(&parsed, "Date").unwrap_or_default(),
            subject: get_header(&parsed, "Subject").unwrap_or_default(),
            body_text: extract_body_text(&parsed),
            body_html: extract_body_html(&parsed),
            attachments: extract_attachments(&parsed),
            in_reply_to: get_header(&parsed, "In-Reply-To"),
            references: parse_references(&parsed),
            flags,
        })
    }

    async fn fetch_attachment(
        &self,
        folder: &str,
        uid: u32,
        part_id: &str,
        encoding: Option<&str>,
    ) -> Result<Vec<u8>, MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;

        select_mailbox(session, folder).await?;

        let sequence = format!("{uid}");
        let query = format!("BODY[{part_id}]");
        let mut stream = session
            .uid_fetch(&sequence, &query)
            .await
            .map_err(|e| MailError::Protocol(format!("UID FETCH attachment: {e}")))?;

        let fetch = stream
            .next()
            .await
            .ok_or(MailError::MessageNotFound(format!(
                "attachment {part_id} not found in message {uid}"
            )))?
            .map_err(|e| MailError::Protocol(format!("fetch: {e}")))?;

        let path = SectionPath::Part(
            part_id
                .split('.')
                .map(|n| n.parse::<u32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MailError::Protocol(format!("invalid part_id {part_id}: {e}")))?,
            None,
        );

        let raw = fetch.section(&path).unwrap_or(&[]);
        Ok(decode_transfer_encoding(raw, encoding))
    }

    // ── 发送 ────────────────────────────────────────────────────────

    async fn send(&self, email: &OutgoingEmail) -> Result<(), MailError> {
        let inner = self.inner.lock().await;
        let config = inner.config.as_ref().ok_or(MailError::NotConnected)?;
        SmtpSender::send(config, email).await
    }

    // ── 标记 ────────────────────────────────────────────────────────

    async fn add_flags(
        &self,
        folder: &str,
        uids: &[u32],
        flags: &[MailFlag],
    ) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;
        select_mailbox(session, folder).await?;

        let uid_list = join_uids(uids);
        let flag_list = flags
            .iter()
            .map(mail_flag_to_imap)
            .collect::<Vec<_>>()
            .join(" ");
        let query = format!("+FLAGS ({flag_list})");

        drain_store(session, &uid_list, &query).await
    }

    async fn remove_flags(
        &self,
        folder: &str,
        uids: &[u32],
        flags: &[MailFlag],
    ) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;
        select_mailbox(session, folder).await?;

        let uid_list = join_uids(uids);
        let flag_list = flags
            .iter()
            .map(mail_flag_to_imap)
            .collect::<Vec<_>>()
            .join(" ");
        let query = format!("-FLAGS ({flag_list})");

        drain_store(session, &uid_list, &query).await
    }

    // ── 移动 / 复制 ─────────────────────────────────────────────────

    async fn move_messages(
        &self,
        from_folder: &str,
        to_folder: &str,
        uids: &[u32],
    ) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;
        select_mailbox(session, from_folder).await?;

        let uid_list = join_uids(uids);
        session
            .uid_mv(&uid_list, to_folder)
            .await
            .map_err(|e| MailError::Protocol(format!("UID MOVE: {e}")))?;

        Ok(())
    }

    async fn copy_messages(
        &self,
        from_folder: &str,
        to_folder: &str,
        uids: &[u32],
    ) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;
        select_mailbox(session, from_folder).await?;

        let uid_list = join_uids(uids);
        session
            .uid_copy(&uid_list, to_folder)
            .await
            .map_err(|e| MailError::Protocol(format!("UID COPY: {e}")))?;

        Ok(())
    }

    // ── 统计 ────────────────────────────────────────────────────────

    async fn message_count(&self, folder: &str) -> Result<u32, MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;

        let mailbox = session
            .status(folder, "(MESSAGES)")
            .await
            .map_err(|e| MailError::Protocol(format!("STATUS: {e}")))?;

        Ok(mailbox.exists)
    }

    async fn unread_count(&self, folder: &str) -> Result<u32, MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;

        let mailbox = session
            .status(folder, "(UNSEEN)")
            .await
            .map_err(|e| MailError::Protocol(format!("STATUS: {e}")))?;

        Ok(mailbox.unseen.unwrap_or(0))
    }

    // ── 保活 ────────────────────────────────────────────────────────

    async fn noop(&self) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let session = inner.session.as_mut().ok_or(MailError::NotConnected)?;
        session
            .noop()
            .await
            .map_err(|e| MailError::Protocol(format!("NOOP: {e}")))?;
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════
// IMAP 操作辅助
// ════════════════════════════════════════════════════════════════════

use async_imap::types::Mailbox;

async fn select_mailbox(session: &mut ImapSession, folder: &str) -> Result<Mailbox, MailError> {
    debug!("SELECT '{folder}' 前发送 NOOP...");
    match session.noop().await {
        Ok(_) => debug!("NOOP 成功"),
        Err(e) => warn!("NOOP 失败: {e}"),
    }
    debug!("SELECT '{folder}'...");
    session
        .select(folder)
        .await
        .map_err(|e| {
            error!("SELECT '{folder}' 失败: {e}");
            MailError::FolderNotFound(format!("select '{folder}': {e}"))
        })
}

async fn fetch_summaries_by_seq(
    session: &mut ImapSession,
    sequence: &str,
) -> Result<Vec<EmailSummary>, MailError> {
    try_fetch_summaries(session, sequence, false).await
}

async fn fetch_summaries_by_uid(
    session: &mut ImapSession,
    sequence: &str,
) -> Result<Vec<EmailSummary>, MailError> {
    try_fetch_summaries(session, sequence, true).await
}

async fn try_fetch_summaries(
    session: &mut ImapSession,
    sequence: &str,
    by_uid: bool,
) -> Result<Vec<EmailSummary>, MailError> {
    // 优先 ENVELOPE → 降级 BODY.PEEK HEADER → 降级基本字段
    for (i, query) in [
        "(UID FLAGS RFC822.SIZE ENVELOPE)",
        "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM TO CC SUBJECT DATE MESSAGE-ID)])",
        "(UID FLAGS RFC822.SIZE)",
    ]
    .iter()
    .enumerate()
    {
        let method = if by_uid { "UID FETCH" } else { "FETCH" };
        debug!("{method} (尝试 #{i}): {query}");
        let result: Result<Vec<EmailSummary>, MailError> = async {
            let mut stream: Pin<Box<dyn Stream<Item = Result<Fetch, async_imap::error::Error>> + Send + '_>> =
                if by_uid {
                    Box::pin(session.uid_fetch(sequence, query).await.map_err(|e| {
                        MailError::Protocol(format!("UID FETCH: {e}"))
                    })?)
                } else {
                    Box::pin(session.fetch(sequence, query).await.map_err(|e| {
                        MailError::Protocol(format!("FETCH: {e}"))
                    })?)
                };
            let mut summaries = Vec::new();
            while let Some(item) = stream.next().await {
                let fetch: Fetch =
                    item.map_err(|e| MailError::Protocol(format!("fetch: {e}")))?;
                summaries.push(build_summary(&fetch));
            }
            summaries.sort_by(|a, b| b.uid.cmp(&a.uid));
            Ok(summaries)
        }
        .await;

        match result {
            Ok(summaries) => {
                info!("FETCH 成功 (尝试 #{i}): {} 封邮件", summaries.len());
                return Ok(summaries);
            }
            Err(ref e) if query.contains("ENVELOPE") => {
                warn!("ENVELOPE 查询失败，降级重试: {e}");
                continue;
            }
            Err(e) => {
                error!("FETCH 最终失败: {e}");
                return Err(e);
            }
        }
    }
    unreachable!()
}

async fn drain_store(
    session: &mut ImapSession,
    uid_list: &str,
    query: &str,
) -> Result<(), MailError> {
    let mut stream = session
        .uid_store(uid_list, query)
        .await
        .map_err(|e| MailError::Protocol(format!("UID STORE: {e}")))?;

    while let Some(item) = stream.next().await {
        item.map_err(|e| MailError::Protocol(format!("STORE stream: {e}")))?;
    }
    Ok(())
}

fn build_summary(fetch: &Fetch) -> EmailSummary {
    let envelope = fetch.envelope();
    let header = fetch.header().map(bytes_to_string);

    EmailSummary {
        uid: fetch.uid.unwrap_or(0),
        message_id: envelope
            .and_then(|e| e.message_id.as_ref())
            .map(bytes_to_string)
            .or_else(|| parse_header_field(&header, "message-id")),
        from: envelope_from(envelope)
            .or_else(|| parse_header_field(&header, "from"))
            .unwrap_or_default(),
        to: envelope_to(envelope)
            .or_else(|| parse_header_field(&header, "to"))
            .unwrap_or_default(),
        cc: envelope_cc(envelope)
            .or_else(|| parse_header_field(&header, "cc")),
        subject: envelope
            .and_then(|e| e.subject.as_ref())
            .map(bytes_to_string)
            .map(|s| decode_mime_header(&s))
            .or_else(|| parse_header_field(&header, "subject"))
            .unwrap_or_default(),
        date: envelope
            .and_then(|e| e.date.as_ref())
            .map(bytes_to_string)
            .or_else(|| parse_header_field(&header, "date"))
            .unwrap_or_default(),
        size: fetch.size.unwrap_or(0) as u64,
        flags: fetch.flags().map(parse_flag).collect(),
        has_attachments: false,
    }
}

fn format_addr(addr: &Address<'_>) -> String {
    let name = addr.name.as_ref().map(bytes_to_string).map(|s| decode_mime_header(&s));
    let mailbox = addr
        .mailbox
        .as_ref()
        .map(bytes_to_string)
        .unwrap_or_default();
    let host = addr.host.as_ref().map(bytes_to_string).unwrap_or_default();

    if !mailbox.is_empty() && !host.is_empty() {
        match name {
            Some(n) if !n.is_empty() => format!("{n} <{mailbox}@{host}>"),
            _ => format!("{mailbox}@{host}"),
        }
    } else {
        name.unwrap_or_default()
    }
}

fn envelope_from(envelope: Option<&imap_proto::types::Envelope<'_>>) -> Option<String> {
    let list = envelope?.from.as_ref()?;
    let s: String = list.iter().map(format_addr).collect::<Vec<_>>().join(", ");
    if s.is_empty() { None } else { Some(s) }
}

fn envelope_to(envelope: Option<&imap_proto::types::Envelope<'_>>) -> Option<String> {
    let list = envelope?.to.as_ref()?;
    let s: String = list.iter().map(format_addr).collect::<Vec<_>>().join(", ");
    if s.is_empty() { None } else { Some(s) }
}

fn envelope_cc(envelope: Option<&imap_proto::types::Envelope<'_>>) -> Option<String> {
    let list = envelope?.cc.as_ref()?;
    if list.is_empty() { return None; }
    Some(list.iter().map(format_addr).collect::<Vec<_>>().join(", "))
}

fn parse_header_field(raw: &Option<String>, name: &str) -> Option<String> {
    let text = raw.as_ref()?;
    let name_lower = name.to_lowercase();
    for line in text.lines() {
        if let Some((key, val)) = line.split_once(':') {
            if key.trim().to_lowercase() == name_lower {
                return Some(val.trim().to_string());
            }
        }
    }
    None
}

// ════════════════════════════════════════════════════════════════════
// MIME 解析辅助
// ════════════════════════════════════════════════════════════════════

fn get_header(parsed: &ParsedMail<'_>, name: &str) -> Option<String> {
    for h in &parsed.headers {
        if h.get_key_ref().eq_ignore_ascii_case(name) {
            return Some(h.get_value());
        }
    }
    None
}

fn split_addr_list(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn extract_body_text(parsed: &ParsedMail<'_>) -> Option<String> {
    if is_text_plain(parsed) {
        return decode_body_part(parsed);
    }
    for part in &parsed.subparts {
        if is_text_plain(part) {
            return decode_body_part(part);
        }
        if let Some(body) = extract_body_text(part) {
            return Some(body);
        }
    }
    None
}

fn extract_body_html(parsed: &ParsedMail<'_>) -> Option<String> {
    if is_text_html(parsed) {
        return decode_body_part(parsed);
    }
    for part in &parsed.subparts {
        if is_text_html(part) {
            return decode_body_part(part);
        }
        if let Some(body) = extract_body_html(part) {
            return Some(body);
        }
    }
    None
}

/// 解码邮件正文部分：取原始字节（已解 transfer-encoding），
/// 优先按 Content-Type 指定的 charset 解码，若结果含乱码则尝试常见编码
fn decode_body_part(part: &ParsedMail<'_>) -> Option<String> {
    let raw = part.get_body_raw().ok()?;
    let declared_charset = part
        .ctype
        .params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("charset"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    // 先按声明编码解码
    if !declared_charset.is_empty() {
        let result = decode_charset(&raw, declared_charset);
        if !looks_garbled(&result) {
            return Some(result);
        }
    }

    // 声明编码不可靠或未声明 → 依次尝试常见编码
    let candidates: [&'static encoding_rs::Encoding; 5] = [
        encoding_rs::GBK,
        encoding_rs::UTF_8,
        encoding_rs::BIG5,
        encoding_rs::SHIFT_JIS,
        encoding_rs::WINDOWS_1252,
    ];
    let candidate_names = ["GBK", "UTF-8", "Big5", "Shift_JIS", "WINDOWS-1252"];
    for (encoding, name) in candidates.iter().zip(candidate_names.iter()) {
        if declared_charset.eq_ignore_ascii_case(name) {
            continue; // 已经试过了
        }
        let (decoded, _, _) = encoding.decode(&raw);
        let decoded = decoded.into_owned();
        if !looks_garbled(&decoded) {
            return Some(decoded);
        }
    }

    // 全部失败，返回第一个尝试的结果
    let (fallback, _, _) = encoding_rs::GBK.decode(&raw);
    Some(fallback.into_owned())
}

/// 简单启发式检测文本是否包含常见乱码特征
fn looks_garbled(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut bad_count = 0u32;
    let mut total = 0u32;
    for ch in s.chars() {
        total += 1;
        let c = ch as u32;
        // C1 control chars (0x80-0x9F) + ÃÂÂÂÂÂ等字符
        if (0x80..=0x9F).contains(&c)
            || c == 0xC2
            || c == 0xC3
            || c == 0xC4
            || c == 0xC5
            || c == 0xC6
            || c == 0xC7
        {
            bad_count += 1;
        }
    }
    total > 5 && bad_count > total / 4
}

fn extract_attachments(parsed: &ParsedMail<'_>) -> Vec<AttachmentMeta> {
    let mut attachments = Vec::new();
    collect_attachments(parsed, "", &mut attachments);
    attachments
}

fn collect_attachments(
    parsed: &ParsedMail<'_>,
    parent_prefix: &str,
    out: &mut Vec<AttachmentMeta>,
) {
    let ctype = parsed.ctype.mimetype.to_lowercase();

    // 跳过顶层内联正文
    if ctype.starts_with("text/plain") || ctype.starts_with("text/html") {
        let is_attachment = parsed.headers.iter().any(|h| {
            h.get_key_ref().eq_ignore_ascii_case("Content-Disposition")
                && h.get_value().to_lowercase().contains("attachment")
        });

        if !is_attachment {
            for (i, part) in parsed.subparts.iter().enumerate() {
                let prefix = child_prefix(parent_prefix, i);
                collect_attachments(part, &prefix, out);
            }
            return;
        }
    }

    // multipart 容器递归
    if ctype.starts_with("multipart/") {
        for (i, part) in parsed.subparts.iter().enumerate() {
            let prefix = child_prefix(parent_prefix, i);
            collect_attachments(part, &prefix, out);
        }
        return;
    }

    // 这是一个附件
    let filename = parsed
        .headers
        .iter()
        .find(|h| h.get_key_ref().eq_ignore_ascii_case("Content-Disposition"))
        .map(|h| {
            let dis = parse_content_disposition(&h.get_value());
            dis.params.get("filename").cloned()
        })
        .flatten()
        .or_else(|| {
            parsed
                .headers
                .iter()
                .find(|h| h.get_key_ref().eq_ignore_ascii_case("Content-Type"))
                .and_then(|h| {
                    let ct = mailparse::parse_content_type(&h.get_value());
                    ct.params.get("name").cloned()
                })
        })
        .unwrap_or_else(|| format!("part_{}", parent_prefix));

    let content_id = get_header(parsed, "Content-ID");

    // 如果文件名没有后缀，根据 MIME 类型补充
    let filename = if !filename.contains('.') {
        let ext = mime_to_ext(&parsed.ctype.mimetype);
        format!("{filename}.{ext}")
    } else {
        filename
    };

    let encoding = get_header(parsed, "Content-Transfer-Encoding");
    let size = parsed.get_body().map(|b| b.len() as u64).unwrap_or(0);

    out.push(AttachmentMeta {
        filename,
        mime_type: parsed.ctype.mimetype.clone(),
        size,
        content_id,
        part_id: parent_prefix.to_string(),
        transfer_encoding: encoding,
    });
}

/// 根据 MIME 类型返回常见文件后缀
fn mime_to_ext(mime: &str) -> &'static str {
    match mime.to_lowercase().as_str() {
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/x-rar-compressed" => "rar",
        "application/x-7z-compressed" => "7z",
        "application/gzip" => "gz",
        "application/x-tar" => "tar",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.ms-powerpoint" => "ppt",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "text/plain" => "txt",
        "text/html" => "html",
        "text/csv" => "csv",
        "text/calendar" => "ics",
        "application/json" => "json",
        "application/xml" | "text/xml" => "xml",
        "application/octet-stream" => "bin",
        // 如果都不匹配，用 bin
        _ => "bin",
    }
}

fn child_prefix(parent: &str, index: usize) -> String {
    if parent.is_empty() {
        (index + 1).to_string()
    } else {
        format!("{}.{}", parent, index + 1)
    }
}

fn is_text_plain(part: &ParsedMail<'_>) -> bool {
    part.ctype.mimetype.eq_ignore_ascii_case("text/plain")
}

fn is_text_html(part: &ParsedMail<'_>) -> bool {
    part.ctype.mimetype.eq_ignore_ascii_case("text/html")
}

fn parse_references(parsed: &ParsedMail<'_>) -> Vec<String> {
    let raw = get_header(parsed, "References").unwrap_or_default();
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split_whitespace()
        .map(|s| s.trim_matches(&['<', '>'] as &[_]).to_string())
        .collect()
}

// ════════════════════════════════════════════════════════════════════
// 标记转换
// ════════════════════════════════════════════════════════════════════

fn parse_flag(flag: Flag<'_>) -> MailFlag {
    match flag {
        Flag::Seen => MailFlag::Seen,
        Flag::Answered => MailFlag::Answered,
        Flag::Flagged => MailFlag::Flagged,
        Flag::Deleted => MailFlag::Deleted,
        Flag::Draft => MailFlag::Draft,
        Flag::Recent => MailFlag::Recent,
        Flag::MayCreate => MailFlag::Custom("\\*".into()),
        Flag::Custom(s) => MailFlag::Custom(s.into_owned()),
    }
}

fn mail_flag_to_imap(flag: &MailFlag) -> String {
    match flag {
        MailFlag::Seen => "\\Seen".into(),
        MailFlag::Answered => "\\Answered".into(),
        MailFlag::Flagged => "\\Flagged".into(),
        MailFlag::Deleted => "\\Deleted".into(),
        MailFlag::Draft => "\\Draft".into(),
        MailFlag::Recent => "\\Recent".into(),
        MailFlag::Custom(s) => s.clone(),
    }
}

fn name_attr_to_string(attr: &async_imap::types::NameAttribute<'_>) -> String {
    use async_imap::types::NameAttribute;
    match attr {
        NameAttribute::NoInferiors => "\\NoInferiors".into(),
        NameAttribute::NoSelect => "\\NoSelect".into(),
        NameAttribute::Marked => "\\Marked".into(),
        NameAttribute::Unmarked => "\\Unmarked".into(),
        NameAttribute::All => "\\All".into(),
        NameAttribute::Archive => "\\Archive".into(),
        NameAttribute::Drafts => "\\Drafts".into(),
        NameAttribute::Flagged => "\\Flagged".into(),
        NameAttribute::Junk => "\\Junk".into(),
        NameAttribute::Sent => "\\Sent".into(),
        NameAttribute::Trash => "\\Trash".into(),
        NameAttribute::Extension(s) => s.to_string(),
        &_ => format!("{attr:?}"),
    }
}

// ════════════════════════════════════════════════════════════════════
// 通用辅助
// ════════════════════════════════════════════════════════════════════

fn bytes_to_string(bytes: impl AsRef<[u8]>) -> String {
    String::from_utf8_lossy(bytes.as_ref()).into_owned()
}

/// 解码 MIME encoded-word（如 `=?UTF-8?B?5Lit5paH?=` → `中文`）
fn decode_mime_header(raw: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;
    while pos < raw.len() {
        if let Some(start) = raw[pos..].find("=?") {
            result.push_str(&raw[pos..pos + start]);
            let start = pos + start;
            if let Some(end) = raw[start + 2..].find("?=") {
                let end = start + 2 + end + 2;
                let inner = &raw[start + 2..end - 2];
                let parts: Vec<&str> = inner.splitn(3, '?').collect();
                if parts.len() == 3 {
                    let (charset, encoding, data) = (parts[0], parts[1], parts[2]);
                    let decoded = match encoding {
                        "B" | "b" => base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            data.as_bytes(),
                        )
                        .ok()
                        .map(|v| decode_charset(&v, charset)),
                        "Q" | "q" => {
                            let mut bytes = Vec::new();
                            let q_chars: Vec<char> = data.chars().collect();
                            let mut i = 0;
                            while i < q_chars.len() {
                                if q_chars[i] == '=' && i + 2 < q_chars.len() {
                                    if let Ok(b) = u8::from_str_radix(&data[i + 1..i + 3], 16) {
                                        bytes.push(b);
                                        i += 3;
                                        continue;
                                    }
                                } else if q_chars[i] == '_' {
                                    bytes.push(b' ');
                                } else {
                                    bytes.push(q_chars[i] as u8);
                                }
                                i += 1;
                            }
                            Some(decode_charset(&bytes, charset))
                        }
                        _ => None,
                    };
                    if let Some(text) = decoded {
                        result.push_str(&text);
                        pos = end;
                        continue;
                    }
                }
            }
        }
        // 找不到或解码失败，输出剩余
        result.push_str(&raw[pos..]);
        break;
    }
    result
}

/// 按字符集解码字节
fn decode_charset(data: &[u8], charset: &str) -> String {
    let charset = charset.to_uppercase();
    match charset.as_str() {
        "UTF-8" | "UTF8" => String::from_utf8_lossy(data).into_owned(),
        "GBK" | "GB2312" | "GB18030" | "GBK-EUC" | "CSGB2312" => {
            encoding_rs::GBK.decode(data).0.into_owned()
        }
        "ISO-8859-1" | "LATIN1" => data.iter().map(|&b| b as char).collect(),
        "SHIFT_JIS" | "SHIFT-JIS" | "SJIS" | "CSSHIFTJIS" => {
            encoding_rs::SHIFT_JIS.decode(data).0.into_owned()
        }
        "EUC-JP" | "EUCJP" => {
            encoding_rs::EUC_JP.decode(data).0.into_owned()
        }
        "BIG5" | "BIG5-HKSCS" | "CN-BIG5" | "CSBIG5" => {
            encoding_rs::BIG5.decode(data).0.into_owned()
        }
        "WINDOWS-1252" => {
            encoding_rs::WINDOWS_1252.decode(data).0.into_owned()
        }
        "KOI8-R" => {
            encoding_rs::KOI8_R.decode(data).0.into_owned()
        }
        "KOI8-U" => {
            encoding_rs::KOI8_U.decode(data).0.into_owned()
        }
        "ISO-8859-2" => {
            encoding_rs::ISO_8859_2.decode(data).0.into_owned()
        }
        "ISO-8859-5" => {
            encoding_rs::ISO_8859_5.decode(data).0.into_owned()
        }
        "ISO-8859-7" => {
            encoding_rs::ISO_8859_7.decode(data).0.into_owned()
        }
        "ISO-8859-15" => {
            encoding_rs::ISO_8859_15.decode(data).0.into_owned()
        }
        _ => {
            // 尝试用 encoding_rs 的标签查找
            let label = charset.as_str();
            if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
                encoding.decode(data).0.into_owned()
            } else {
                // 兜底：尝试按 UTF-8 解码
                String::from_utf8_lossy(data).into_owned()
            }
        }
    }
}

fn join_uids(uids: &[u32]) -> String {
    uids.iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// 解码 Content-Transfer-Encoding (base64 / quoted-printable)
fn decode_transfer_encoding(data: &[u8], encoding: Option<&str>) -> Vec<u8> {
    match encoding.map(|e| e.to_lowercase()) {
        Some(ref e) if e == "base64" => {
            let clean: String = String::from_utf8_lossy(data)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            base64::engine::general_purpose::STANDARD
                .decode(clean.as_bytes())
                .unwrap_or_else(|_| data.to_vec())
        }
        Some(ref e) if e == "quoted-printable" => quoted_printable_decode(data),
        _ => data.to_vec(),
    }
}

fn quoted_printable_decode(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    let len = data.len();
    while i < len {
        match data[i] {
            b'=' if i + 1 < len => {
                if data[i + 1] == b'\r' && i + 2 < len && data[i + 2] == b'\n' {
                    i += 3; // soft line break =\r\n
                } else if data[i + 1] == b'\n' {
                    i += 2; // soft line break =\n
                } else if i + 2 < len {
                    let hex = &data[i + 1..i + 3];
                    if let Ok(b) = u8::from_str_radix(std::str::from_utf8(hex).unwrap_or("00"), 16)
                    {
                        result.push(b);
                        i += 3;
                    } else {
                        result.push(data[i]);
                        i += 1;
                    }
                } else {
                    result.push(data[i]);
                    i += 1;
                }
            }
            b => {
                result.push(b);
                i += 1;
            }
        }
    }
    result
}



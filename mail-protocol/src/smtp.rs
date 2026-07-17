use lettre::{
    AsyncTransport, Message,
    message::{
        Mailbox, MultiPart, SinglePart,
        header::{ContentDisposition, ContentId, ContentType},
    },
    transport::smtp::{AsyncSmtpTransport, authentication::Credentials},
};
use tracing::{error, info};

use crate::backend::{AccountConfig, OutgoingEmail, SecurityMode};
use crate::error::MailError;

/// SMTP 发送器 — 每次 `send()` 建立短连接, 发送后断开。
pub struct SmtpSender;

impl SmtpSender {
    pub fn new() -> Self {
        Self
    }

    pub async fn send(config: &AccountConfig, email: &OutgoingEmail) -> Result<(), MailError> {
        let message = build_message(email)?;
        let transport = create_transport(config)?;

        info!("SMTP 发送中: {} -> {:?}", config.smtp_host, email.to);
        match transport.send(message).await {
            Ok(_) => {
                info!("SMTP 发送成功");
                Ok(())
            }
            Err(e) => {
                error!("SMTP 发送失败: {e}");
                Err(MailError::Network(format!("SMTP send failed: {e}")))
            }
        }
    }
}

impl Default for SmtpSender {
    fn default() -> Self {
        Self::new()
    }
}

// ── 内部辅助 ────────────────────────────────────────────────────────

fn create_transport(
    config: &AccountConfig,
) -> Result<AsyncSmtpTransport<lettre::Tokio1Executor>, MailError> {
    let credentials = Credentials::new(config.username.clone(), config.password.clone());

    let transport = match config.security {
        SecurityMode::Tls => {
            AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(&config.smtp_host)
                .map_err(|e| MailError::Connection(format!("invalid SMTP relay: {e}")))?
        }
        SecurityMode::StartTls => {
            AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(&config.smtp_host)
                .map_err(|e| MailError::Connection(format!("invalid SMTP relay: {e}")))?
        }
        SecurityMode::None => {
            return Err(MailError::Connection(
                "unencrypted SMTP not supported via lettre relay builder".into(),
            ));
        }
    }
    .credentials(credentials)
    .build();

    Ok(transport)
}

fn build_message(email: &OutgoingEmail) -> Result<Message, MailError> {
    let from: Mailbox = email
        .from
        .parse()
        .map_err(|e| MailError::Parse(format!("invalid from '{}': {e}", email.from)))?;

    let mut builder = Message::builder().from(from).subject(email.subject.clone());

    if let Some(ref in_reply_to) = email.in_reply_to {
        builder = builder.in_reply_to(in_reply_to.clone());
    }

    for ref_id in &email.references {
        builder = builder.references(ref_id.clone());
    }

    for to in &email.to {
        let addr: Mailbox = to
            .parse()
            .map_err(|e| MailError::Parse(format!("invalid to '{to}': {e}")))?;
        builder = builder.to(addr);
    }

    for cc in &email.cc {
        let addr: Mailbox = cc
            .parse()
            .map_err(|e| MailError::Parse(format!("invalid cc '{cc}': {e}")))?;
        builder = builder.cc(addr);
    }

    for bcc in &email.bcc {
        let addr: Mailbox = bcc
            .parse()
            .map_err(|e| MailError::Parse(format!("invalid bcc '{bcc}': {e}")))?;
        builder = builder.bcc(addr);
    }

    let mime = build_mime(email)?;
    let message = builder
        .multipart(mime)
        .map_err(|e| MailError::Parse(format!("failed to build message: {e}")))?;

    Ok(message)
}

fn build_mime(email: &OutgoingEmail) -> Result<MultiPart, MailError> {
    let mut parts = Vec::new();

    if let Some(ref text) = email.body_text {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            parts.push(
                SinglePart::builder()
                    .header(ContentType::TEXT_PLAIN)
                    .body(trimmed.to_string()),
            );
        }
    }

    if let Some(ref html) = email.body_html {
        let trimmed = html.trim();
        if !trimmed.is_empty() {
            parts.push(
                SinglePart::builder()
                    .header(ContentType::TEXT_HTML)
                    .body(trimmed.to_string()),
            );
        }
    }

    for att in &email.attachments {
        let content_type = ContentType::parse(&att.mime_type)
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap());

        let mut part_builder = SinglePart::builder()
            .header(content_type)
            .header(ContentDisposition::attachment(&att.filename));

        if let Some(ref cid) = att.content_id {
            part_builder = part_builder.header(ContentId::from(format!("<{cid}>")));
        }

        parts.push(part_builder.body(att.data.clone()));
    }

    let mut mixed = MultiPart::mixed().build();
    for part in parts {
        mixed = mixed.singlepart(part);
    }

    Ok(mixed)
}

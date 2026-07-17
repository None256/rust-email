use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::MailError;

// ── 配置 ────────────────────────────────────────────────────────────

/// 账户连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// IMAP 接收服务器地址
    pub imap_host: String,
    /// IMAP 端口 (993 = TLS, 143 = STARTTLS)
    pub imap_port: u16,
    /// SMTP 发送服务器地址
    pub smtp_host: String,
    /// SMTP 端口 (465 = TLS, 587 = STARTTLS)
    pub smtp_port: u16,
    /// 登录用户名
    pub username: String,
    /// 登录密码或 OAuth2 access token
    pub password: String,
    /// 连接安全模式
    pub security: SecurityMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityMode {
    /// 直接 TLS 连接 (IMAP 993, SMTP 465)
    Tls,
    /// 先普通连接, 再 STARTTLS 升级 (IMAP 143, SMTP 587)
    StartTls,
    /// 不加密
    None,
}

// ── 文件夹 ──────────────────────────────────────────────────────────

/// 邮箱文件夹
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    /// 文件夹名称 (如 "INBOX", "Sent", "Archive")
    pub name: String,
    /// 层级分隔符 (IMAP 中通常为 "/" 或 ".")
    pub delimiter: String,
    /// IMAP 属性列表: \HasChildren, \HasNoChildren, \NoSelect, \Sent 等
    pub attributes: Vec<String>,
}

// ── 邮件 ────────────────────────────────────────────────────────────

/// 邮件列表项 — 仅含摘要信息, 不包含正文, 用于列表展示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailSummary {
    /// IMAP UID (不可变唯一标识)
    pub uid: u32,
    /// Message-ID 头
    pub message_id: Option<String>,
    /// 发件人
    pub from: String,
    /// 收件人
    pub to: String,
    /// 抄送
    pub cc: Option<String>,
    /// 主题
    pub subject: String,
    /// 发送日期 (RFC 2822)
    pub date: String,
    /// 邮件大小 (字节)
    pub size: u64,
    /// 标记
    pub flags: Vec<MailFlag>,
    /// 是否有附件
    pub has_attachments: bool,
}

/// 完整邮件 — 含正文和附件元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Email {
    /// IMAP UID
    pub uid: u32,
    /// 所属文件夹名
    pub folder: String,
    /// Message-ID 头
    pub message_id: Option<String>,
    /// 发件人
    pub from: String,
    /// 收件人
    pub to: Vec<String>,
    /// 抄送
    pub cc: Vec<String>,
    /// 密送 (仅自己发出的邮件有值)
    pub bcc: Vec<String>,
    /// Reply-To 头
    pub reply_to: Option<String>,
    /// 发送日期 (RFC 2822)
    pub date: String,
    /// 主题
    pub subject: String,
    /// 纯文本正文
    pub body_text: Option<String>,
    /// HTML 正文
    pub body_html: Option<String>,
    /// 附件元信息 (不含文件内容, 内容按需下载)
    pub attachments: Vec<AttachmentMeta>,
    /// In-Reply-To 头
    pub in_reply_to: Option<String>,
    /// References 头
    pub references: Vec<String>,
    /// 当前标记
    pub flags: Vec<MailFlag>,
}

/// 附件元信息 — 接收邮件中的附件描述
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    /// 文件名
    pub filename: String,
    /// MIME 类型
    pub mime_type: String,
    /// 大小 (字节, 解码后)
    pub size: u64,
    /// Content-ID (内嵌图片引用)
    pub content_id: Option<String>,
    /// IMAP body part 标识, 用于按需下载附件内容
    pub part_id: String,
    /// Content-Transfer-Encoding (base64 / quoted-printable / 7bit / 8bit)
    pub transfer_encoding: Option<String>,
}

/// 附件数据 — 发送邮件时的实际附件内容
#[derive(Debug, Clone)]
pub struct AttachmentData {
    /// 文件名
    pub filename: String,
    /// MIME 类型
    pub mime_type: String,
    /// 文件内容
    pub data: Vec<u8>,
    /// Content-ID (内嵌图片引用)
    pub content_id: Option<String>,
}

/// 待发送邮件
#[derive(Debug, Clone)]
pub struct OutgoingEmail {
    /// 发件人
    pub from: String,
    /// 收件人
    pub to: Vec<String>,
    /// 抄送
    pub cc: Vec<String>,
    /// 密送
    pub bcc: Vec<String>,
    /// 主题
    pub subject: String,
    /// 纯文本正文
    pub body_text: Option<String>,
    /// HTML 正文
    pub body_html: Option<String>,
    /// 附件
    pub attachments: Vec<AttachmentData>,
    /// 回复邮件的 Message-ID
    pub in_reply_to: Option<String>,
    /// 引用邮件的 Message-ID 列表
    pub references: Vec<String>,
}

// ── 标记 ────────────────────────────────────────────────────────────

/// 邮件标记
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MailFlag {
    /// 已读 (\Seen)
    Seen,
    /// 已回复 (\Answered)
    Answered,
    /// 星标 (\Flagged)
    Flagged,
    /// 已删除 (\Deleted)
    Deleted,
    /// 草稿 (\Draft)
    Draft,
    /// 最近到达 (\Recent)
    Recent,
    /// 自定义标签 (如 Gmail 标签)
    Custom(String),
}

// ── 协议抽象 ────────────────────────────────────────────────────────

/// 邮件协议后端
///
/// 所有网络 I/O 通过此 trait 进行。
/// UI 和数据库层只依赖这个接口, 不关心底层是 IMAP/SMTP 还是其他协议。
#[async_trait]
pub trait MailBackend: Send + Sync {
    // ---- 连接生命周期 ----

    /// 连接到邮件服务器 (同时建立 IMAP 和 SMTP 连接)
    async fn connect(&mut self, config: &AccountConfig) -> Result<(), MailError>;

    /// 断开所有连接
    async fn disconnect(&mut self) -> Result<(), MailError>;

    /// 当前是否已连接
    fn is_connected(&self) -> bool;

    // ---- 文件夹操作 ----

    /// 获取所有文件夹
    async fn list_folders(&self) -> Result<Vec<Folder>, MailError>;

    /// 创建文件夹
    async fn create_folder(&self, name: &str) -> Result<(), MailError>;

    /// 删除文件夹
    async fn delete_folder(&self, name: &str) -> Result<(), MailError>;

    /// 重命名文件夹
    async fn rename_folder(&self, old_name: &str, new_name: &str) -> Result<(), MailError>;

    // ---- 邮件列表 (分页加载) ----

    /// 获取文件夹中最新的 N 封邮件摘要 (按日期降序)
    async fn fetch_latest_messages(
        &self,
        folder: &str,
        count: u32,
    ) -> Result<Vec<EmailSummary>, MailError>;

    /// 获取指定 UID 之前的 N 封邮件摘要 (向下翻页用)
    async fn fetch_messages_before(
        &self,
        folder: &str,
        before_uid: u32,
        count: u32,
    ) -> Result<Vec<EmailSummary>, MailError>;

    /// 获取 UID 大于指定值的所有新邮件 (刷新用)
    async fn fetch_new_messages(
        &self,
        folder: &str,
        since_uid: u32,
    ) -> Result<Vec<EmailSummary>, MailError>;

    // ---- 邮件内容 ----

    /// 获取指定邮件的完整内容 (含正文和附件元信息)
    async fn fetch_message(&self, folder: &str, uid: u32) -> Result<Email, MailError>;

    /// 下载指定附件的原始内容
    /// `encoding`: Content-Transfer-Encoding, 用于解码原始数据
    async fn fetch_attachment(
        &self,
        folder: &str,
        uid: u32,
        part_id: &str,
        encoding: Option<&str>,
    ) -> Result<Vec<u8>, MailError>;

    // ---- 发送 ----

    /// 发送邮件
    async fn send(&self, email: &OutgoingEmail) -> Result<(), MailError>;

    // ---- 标记 ----

    /// 添加标记
    async fn add_flags(
        &self,
        folder: &str,
        uids: &[u32],
        flags: &[MailFlag],
    ) -> Result<(), MailError>;

    /// 移除标记
    async fn remove_flags(
        &self,
        folder: &str,
        uids: &[u32],
        flags: &[MailFlag],
    ) -> Result<(), MailError>;

    // ---- 移动 / 复制 ----

    /// 移动邮件到另一个文件夹
    async fn move_messages(
        &self,
        from_folder: &str,
        to_folder: &str,
        uids: &[u32],
    ) -> Result<(), MailError>;

    /// 复制邮件到另一个文件夹
    async fn copy_messages(
        &self,
        from_folder: &str,
        to_folder: &str,
        uids: &[u32],
    ) -> Result<(), MailError>;

    // ---- 统计 ----

    /// 文件夹中的邮件总数
    async fn message_count(&self, folder: &str) -> Result<u32, MailError>;

    /// 文件夹中的未读邮件数
    async fn unread_count(&self, folder: &str) -> Result<u32, MailError>;

    // ---- 保活 ----

    /// 发送 NOOP 保持连接, 同时检查邮箱状态变化
    async fn noop(&self) -> Result<(), MailError>;
}

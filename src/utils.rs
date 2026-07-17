/// 解码 IMAP modified UTF-7 文件夹名并翻译为中文显示名
pub fn folder_display_name(name: &str) -> String {
    let decoded = utf7_imap::decode_utf7_imap(name.to_string());
    match decoded.as_str() {
        "INBOX" => "收件箱".into(),
        "Sent" | "Sent Messages" | "Sent Items" => "已发送".into(),
        "Drafts" => "草稿箱".into(),
        "Trash" | "Deleted Messages" | "Deleted Items" => "垃圾箱".into(),
        "Junk" | "Spam" | "Junk Email" => "垃圾邮件".into(),
        "Archive" | "Archives" => "归档".into(),
        "Outbox" => "发件箱".into(),
        "Important" => "重要邮件".into(),
        "Flagged" | "Starred" => "星标邮件".into(),
        _ => decoded,
    }
}

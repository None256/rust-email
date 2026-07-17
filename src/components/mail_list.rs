use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use tokio::sync::mpsc::UnboundedSender;

use super::Component;
use crate::action::Action;

#[derive(Default)]
pub struct MailList {
    command_tx: Option<UnboundedSender<Action>>,
    pub mails: Vec<mail_protocol::EmailSummary>,
    pub state: ListState,
    pub current_folder: String,
}

impl MailList {
    pub fn new() -> Self {
        Self {
            state: ListState::default().with_selected(Some(0)),
            ..Self::default()
        }
    }

    pub fn set_mails(&mut self, mails: Vec<mail_protocol::EmailSummary>) {
        self.mails = mails;
        self.state.select(if self.mails.is_empty() {
            None
        } else {
            Some(0)
        });
    }

    pub fn selected_uid(&self) -> Option<u32> {
        self.state
            .selected()
            .and_then(|i| self.mails.get(i))
            .map(|m| m.uid)
    }

    /// 切换星标（返回 true=已加星, false=取消星标）
    pub fn toggle_flag(&mut self) -> Option<(u32, bool)> {
        let i = self.state.selected()?;
        let mail = self.mails.get_mut(i)?;
        let flagged = mail.flags.contains(&mail_protocol::MailFlag::Flagged);
        if flagged {
            mail.flags.retain(|f| *f != mail_protocol::MailFlag::Flagged);
        } else {
            mail.flags.push(mail_protocol::MailFlag::Flagged);
        }
        Some((mail.uid, !flagged))
    }

    fn next(&mut self) {
        let i = self
            .state
            .selected()
            .map(|i| (i + 1).min(self.mails.len().saturating_sub(1)))
            .unwrap_or(0);
        self.state.select(Some(i));
    }

    fn prev(&mut self) {
        let i = self
            .state
            .selected()
            .map(|i| i.saturating_sub(1))
            .unwrap_or(0);
        self.state.select(Some(i));
    }
}

impl Component for MailList {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) -> color_eyre::Result<Option<Action>> {
        match key.code {
            crossterm::event::KeyCode::Char('j') | crossterm::event::KeyCode::Down => {
                self.next();
            }
            crossterm::event::KeyCode::Char('k') | crossterm::event::KeyCode::Up => {
                self.prev();
            }
            crossterm::event::KeyCode::Enter => {
                if self.selected_uid().is_some() {
                    return Ok(Some(Action::ViewMail));
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn update(&mut self, action: Action) -> color_eyre::Result<Option<Action>> {
        match action {
            Action::SelectFolder(ref name) => {
                self.current_folder = name.clone();
            }
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" 📨 {} ", folder_display_name(&self.current_folder)))
            .style(Style::default().fg(Color::White));
        let inner = block.inner(area);

        // 布局：标记(2) + 发件人(14) + 日期(10) + 空格(1) + 主题 = inner.width
        let show_date = inner.width >= 70;
        let from_w: usize = 14;
        let date_w: usize = if show_date { 10 } else { 0 };
        let subj_w = (inner.width as usize).saturating_sub(from_w + date_w + 4);

        let items: Vec<ListItem> = self
            .mails
            .iter()
            .map(|m| {
                let is_seen = m.flags.contains(&mail_protocol::MailFlag::Seen);
                let attach = if m.has_attachments { " 📎" } else { "" };

                let flag = if m.flags.contains(&mail_protocol::MailFlag::Flagged) {
                    "★"
                } else if is_seen {
                    " "
                } else {
                    "●"
                };

                let display_from = shorten_sender(&m.from);
                let from = truncate_cols(&display_from, from_w);
                let from_padded = pad_right(&from, from_w);

                let date = reformat_date(&m.date);
                let date_padded = pad_left(&date, date_w);

                let subj = truncate_cols(&m.subject, subj_w);
                let subj_padded = pad_right(&subj, subj_w);

                let style = if is_seen {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                };

                let mut spans = vec![
                    Span::styled(format!("{} ", flag), Style::default().fg(Color::Yellow)),
                    Span::styled(from_padded, Style::default().fg(Color::Cyan)),
                ];
                if show_date {
                    spans.push(Span::styled(date_padded, Style::default().fg(Color::DarkGray)));
                }
                spans.push(Span::raw(" "));
                spans.push(Span::styled(subj_padded, style));
                spans.push(Span::raw(attach));

                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        frame.render_stateful_widget(list, area, &mut self.state);

        if self.mails.is_empty() {
            frame.render_widget(
                Paragraph::new(Text::from("暂无邮件\n按 Esc 返回文件夹列表"))
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(ratatui::layout::Alignment::Center),
                inner,
            );
        }

        Ok(())
    }
}

/// 从 "Name <email>" 中提取名字，如果没有名字则返回完整字符串
fn shorten_sender(s: &str) -> String {
    if let Some(pos) = s.find('<') {
        let name = s[..pos].trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    // 没有名字，直接返回邮箱（取 @ 前部分）
    if let Some(at) = s.find('@') {
        s[..at].to_string()
    } else {
        s.to_string()
    }
}

/// 文件夹名解码+翻译（仅供显示，不影响原始名）
fn folder_display_name(name: &str) -> String {
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

/// 计算字符串在终端中的显示宽度（中文 = 2，英文 = 1）
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if is_cjk(c) { 2 } else { 1 }).sum()
}

fn is_cjk(c: char) -> bool {
    c >= '\u{4E00}' && c <= '\u{9FFF}' || c >= '\u{3400}' && c <= '\u{4DBF}'
        || c >= '\u{2E80}' && c <= '\u{2EFF}' || c >= '\u{3000}' && c <= '\u{303F}'
        || c >= '\u{FF00}' && c <= '\u{FFEF}'
}

/// 截断到指定显示宽度
fn truncate_cols(s: &str, max_cols: usize) -> String {
    if max_cols < 2 {
        return String::new();
    }
    let mut cols = 0;
    let mut result = String::new();
    for c in s.chars() {
        let w = if is_cjk(c) { 2 } else { 1 };
        if cols + w > max_cols - 1 {
            result.push('…');
            break;
        }
        result.push(c);
        cols += w;
    }
    result
}

/// 右填充到指定显示宽度（左对齐）
fn pad_right(s: &str, total_cols: usize) -> String {
    let w = display_width(s);
    if w >= total_cols { return s.to_string(); }
    format!("{}{}", s, " ".repeat(total_cols - w))
}

/// 左填充到指定显示宽度（右对齐）
fn pad_left(s: &str, total_cols: usize) -> String {
    let w = display_width(s);
    if w >= total_cols { return s.to_string(); }
    format!("{}{}", " ".repeat(total_cols - w), s)
}

/// "17-Jul-2026" / "9 Jul 2026" / "Thu, 9 Jul 2026 ..." → "2026/07/17"
fn reformat_date(s: &str) -> String {
    let s = s.trim();
    // 跳过 "Thu, " 前缀
    let bytes = s.as_bytes();
    let date_str = if bytes.len() > 4 && bytes[3] == b',' { &s[5..] } else { s };

    // 提取 "dd-Mmm-yyyy" 或 "d Mmm yyyy"
    let parts: Vec<&str> = date_str.split(|c: char| c == '-' || c == ' ' || c == '/')
        .filter(|p| !p.is_empty())
        .collect();

    if parts.len() < 3 { return date_str.to_string(); }

    let day = parts[0];
    let month = match &parts[1][..3.min(parts[1].len())] {
        "Jan" => "01", "Feb" => "02", "Mar" => "03", "Apr" => "04",
        "May" => "05", "Jun" => "06", "Jul" => "07", "Aug" => "08",
        "Sep" => "09", "Oct" => "10", "Nov" => "11", "Dec" => "12",
        _ => return date_str.to_string(),
    };
    let year = parts[2];
    format!("{year}/{month}/{day:0>2}")
}

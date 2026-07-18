use ratatui::{
    Frame,
    layout::{Margin, Rect},
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
    cached_items: Vec<ListItem<'static>>,
    dirty: bool,
    last_width: u16,
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
        self.dirty = true;
    }

    pub fn selected_uid(&self) -> Option<u32> {
        self.state
            .selected()
            .and_then(|i| self.mails.get(i))
            .map(|m| m.uid)
    }

    /// 删除当前选中邮件，返回被删除邮件的 UID
    pub fn remove_selected(&mut self) -> Option<u32> {
        let i = self.state.selected()?;
        let uid = self.mails.get(i)?.uid;
        self.mails.remove(i);
        if self.mails.is_empty() {
            self.state.select(None);
        } else if i >= self.mails.len() {
            self.state.select(Some(self.mails.len() - 1));
        }
        self.dirty = true;
        Some(uid)
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
        self.dirty = true;
        Some((mail.uid, !flagged))
    }

    fn next(&mut self) {
        let i = self
            .state
            .selected()
            .map(|i| (i + 1).min(self.mails.len().saturating_sub(1)))
            .unwrap_or(0);
        self.state.select(Some(i));
        self.dirty = true;
    }

    fn prev(&mut self) {
        let i = self
            .state
            .selected()
            .map(|i| i.saturating_sub(1))
            .unwrap_or(0);
        self.state.select(Some(i));
        self.dirty = true;
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
                self.dirty = true;
            }
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if self.dirty || inner.width != self.last_width {
            self.cached_items = build_items(&self.mails, inner.width);
            self.dirty = false;
            self.last_width = inner.width;
        }

        let list = List::new(self.cached_items.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" 📨 {} ", folder_display_name(&self.current_folder)))
                    .style(Style::default().fg(Color::White)),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        frame.render_stateful_widget(list, area, &mut self.state);

        if self.mails.is_empty() {
            let inner = area.inner(Margin {
                horizontal: 1,
                vertical: 1,
            });
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

fn build_items(mails: &[mail_protocol::EmailSummary], width: u16) -> Vec<ListItem<'static>> {
    let show_date = width >= 70;
    let from_w: usize = 14;
    let date_w: usize = if show_date { 10 } else { 0 };
    let subj_w = (width as usize).saturating_sub(from_w + date_w + 4);

    mails
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
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            };

            let mut spans = vec![
                Span::styled(format!("{} ", flag), Style::default().fg(Color::Yellow)),
                Span::styled(from_padded, Style::default().fg(Color::Cyan)),
            ];
            if show_date {
                spans.push(Span::styled(
                    date_padded,
                    Style::default().fg(Color::DarkGray),
                ));
            }
            spans.push(Span::raw(" "));
            spans.push(Span::styled(subj_padded, style));
            spans.push(Span::raw(attach));

            ListItem::new(Line::from(spans))
        })
        .collect()
}

fn folder_display_name(name: &str) -> String {
    crate::utils::folder_display_name(name)
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

/// RFC 2822 日期 → 北京时间 "2006/07/18"
fn reformat_date(s: &str) -> String {
    use chrono::{DateTime, Datelike, FixedOffset};
    if let Ok(dt) = DateTime::parse_from_rfc2822(s.trim()) {
        let bj = dt.with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap());
        return format!("{}/{:02}/{:02}", bj.year(), bj.month(), bj.day());
    }
    s.to_string()
}

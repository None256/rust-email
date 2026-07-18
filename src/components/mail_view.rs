use crossterm::event::{KeyCode, KeyEvent};
use mail_protocol::Email;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::mpsc::UnboundedSender;

use super::Component;
use crate::action::Action;

#[derive(Default)]
pub struct MailView {
    command_tx: Option<UnboundedSender<Action>>,
    pub mail: Option<Email>,
    pub current_folder: String,
    show_html: bool,
    scroll: u16,
    /// 附件列表选中索引
    attachment_idx: usize,
}

impl MailView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_mail(&mut self, mail: Email) {
        self.mail = Some(mail);
        self.scroll = 0;
        self.attachment_idx = 0;
    }

    fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }
}

fn format_mail_date(raw: &str) -> String {
    use chrono::{DateTime, FixedOffset, Datelike, Timelike, Weekday};
    if let Ok(dt) = DateTime::parse_from_rfc2822(raw) {
        let bj = dt.with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap());
        let wd = match bj.weekday() {
            Weekday::Mon => "周一",
            Weekday::Tue => "周二",
            Weekday::Wed => "周三",
            Weekday::Thu => "周四",
            Weekday::Fri => "周五",
            Weekday::Sat => "周六",
            Weekday::Sun => "周日",
        };
        return format!(
            "{}/{:02}/{:02} {} {:02}:{:02}",
            bj.year(),
            bj.month(),
            bj.day(),
            wd,
            bj.hour(),
            bj.minute(),
        );
    }
    raw.to_string()
}

/// 解码常见 HTML 实体，并将 <br> 替换为换行
fn decode_html_entities(text: &str) -> String {
    let s = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", "\u{00a0}")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    // 解码数字实体 &#ddd; (十进制) 和 &#xHH; (十六进制)
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if chars[i] == '&' && i + 3 < len && chars[i + 1] == '#' {
            let start = i + 2;
            let (is_hex, num_start) = if chars[start] == 'x' {
                (true, start + 1)
            } else {
                (false, start)
            };
            let mut j = num_start;
            while j < len && chars[j] != ';' {
                j += 1;
            }
            if j < len && j > num_start {
                let num_str: String = chars[num_start..j].iter().collect();
                if let Ok(code) = if is_hex {
                    u32::from_str_radix(&num_str, 16)
                } else {
                    num_str.parse::<u32>()
                } {
                    if let Some(c) = char::from_u32(code) {
                        result.push(c);
                        i = j + 1;
                        continue;
                    }
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

impl Component for MailView {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<Option<Action>> {
        let has_attachments = self.mail.as_ref().map_or(false, |m| !m.attachments.is_empty());
        let att_count = self.mail.as_ref().map_or(0, |m| m.attachments.len());

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll_down();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_up();
            }
            KeyCode::Char('h') => {
                self.show_html = true;
            }
            KeyCode::Char('t') => {
                self.show_html = false;
            }
            KeyCode::Char('E') => {
                if self.mail.as_ref().map_or(false, |m| m.body_html.is_some()) {
                    return Ok(Some(Action::SaveHtml));
                }
            }

            // 附件操作
            KeyCode::Char('o') if has_attachments => {
                return Ok(Some(Action::DownloadAttachment(self.attachment_idx)));
            }
            KeyCode::Left if has_attachments && self.attachment_idx > 0 => {
                self.attachment_idx -= 1;
            }
            KeyCode::Right if has_attachments && self.attachment_idx + 1 < att_count => {
                self.attachment_idx += 1;
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
        let Some(ref mail) = self.mail else {
            frame.render_widget(
                Paragraph::new(Text::from("未选择邮件"))
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(ratatui::layout::Alignment::Center),
                area,
            );
            return Ok(());
        };

        let has_attachments = !mail.attachments.is_empty();

        // 分区：邮件头 + 正文 + (附件区)
        let constraints: Vec<Constraint> = if has_attachments {
            vec![
                Constraint::Length(5),  // 邮件头
                Constraint::Min(7),     // 正文
                Constraint::Length(3 + mail.attachments.len().min(5) as u16), // 附件区
            ]
        } else {
            vec![
                Constraint::Length(5),  // 邮件头
                Constraint::Min(1),     // 正文
            ]
        };
        let chunks = Layout::vertical(constraints).split(area);
        let (header_area, body_area) = (chunks[0], chunks[1]);

        // ── 邮件头 ──
        let header_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" 📄 {} ", mail.subject))
            .style(Style::default().fg(Color::Cyan));

        let from_display = if !mail.from.is_empty() {
            mail.from.clone()
        } else {
            "(未知发件人)".to_string()
        };

        let to_display = if mail.to.is_empty() {
            "(无收件人)".to_string()
        } else {
            mail.to.join("; ")
        };

        let header_text = Text::from(vec![
            Line::from(Span::styled(
                format!(" 发件人: {from_display}"),
                Style::default().fg(Color::White),
            )),
            Line::from(Span::styled(
                format!(" 收件人: {to_display}"),
                Style::default().fg(Color::White),
            )),
            Line::from(Span::styled(
                format!(" 日  期: {}", format_mail_date(&mail.date)),
                Style::default().fg(Color::DarkGray),
            )),
        ]);

        frame.render_widget(
            Paragraph::new(header_text)
                .block(header_block)
                .wrap(Wrap { trim: false }),
            header_area,
        );

        // ── 正文 ──
        let body_text = mail.body_text.as_deref().unwrap_or("");
        let has_readable_text = {
            let trimmed = body_text.trim();
            !trimmed.is_empty() && trimmed.len() > 3
        };

        let (body_content, is_html_only) = if self.show_html {
            (mail.body_html.as_deref().unwrap_or("(无 HTML 正文)").to_string(), false)
        } else if has_readable_text {
            (decode_html_entities(body_text), false)
        } else if mail.body_html.is_some() {
            let hint = vec![
                "╔══════════════════════════════════════╗",
                "║                                      ║",
                "║  此邮件仅包含 HTML 格式内容          ║",
                "║                                      ║",
                "║  按 h 键切换到 HTML 视图查看         ║",
                "║  按 E 键保存为 .html 文件到桌面      ║",
                "║                                      ║",
                "╚══════════════════════════════════════╝",
            ];
            (hint.join("\n"), true)
        } else {
            ("(无正文)".to_string(), false)
        };

        let body_block = Block::default()
            .borders(Borders::ALL)
            .title(if self.show_html { " 🌐 HTML" } else if is_html_only { " ⚠ 仅 HTML" } else { " 📝 纯文本" });

        frame.render_widget(
            Paragraph::new(body_content)
                .block(body_block)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            body_area,
        );

        // ── 附件区 ──
        if has_attachments {
            let att_lines: Vec<Line> = mail
                .attachments
                .iter()
                .enumerate()
                .map(|(i, att)| {
                    let selected = i == self.attachment_idx;
                    let prefix = if selected { "▶ " } else { "  " };
                    let size = format_size(att.size);
                    let line = format!("{prefix}[{i}] {}  ({size})", att.filename);
                    if selected {
                        Line::from(Span::styled(
                            line,
                            Style::default().fg(Color::Yellow),
                        ))
                    } else {
                        Line::from(Span::styled(line, Style::default().fg(Color::White)))
                    }
                })
                .collect();

            let hint = Line::from(Span::styled(
                " ←→ 选择附件   o 保存选中附件   保存路径: 桌面/rust-email-attachments/",
                Style::default().fg(Color::DarkGray),
            ));

            let mut all_lines = att_lines;
            all_lines.push(Line::from(""));
            all_lines.push(hint);

            frame.render_widget(
                Paragraph::new(Text::from(all_lines))
                    .block(Block::default().borders(Borders::ALL).title(" 📎 附件 "))
                    .wrap(Wrap { trim: false }),
                chunks[2],
            );
        }

        Ok(())
    }
}

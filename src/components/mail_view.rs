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
    pub mail: Option<mail_protocol::Email>,
    pub current_folder: String,
    show_html: bool,
    scroll: u16,
}

impl MailView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_mail(&mut self, mail: mail_protocol::Email) {
        self.mail = Some(mail);
        self.scroll = 0;
    }

    fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }
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
            // 找到结束的 ;
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

impl Component for MailView {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) -> color_eyre::Result<Option<Action>> {
        match key.code {
            crossterm::event::KeyCode::Char('j') | crossterm::event::KeyCode::Down => {
                self.scroll_down();
            }
            crossterm::event::KeyCode::Char('k') | crossterm::event::KeyCode::Up => {
                self.scroll_up();
            }
            crossterm::event::KeyCode::Char('h') => {
                self.show_html = true;
            }
            crossterm::event::KeyCode::Char('t') => {
                self.show_html = false;
            }

            crossterm::event::KeyCode::Char('E') => {
                if self.mail.as_ref().map_or(false, |m| m.body_html.is_some()) {
                    return Ok(Some(Action::SaveHtml));
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
        let Some(ref mail) = self.mail else {
            frame.render_widget(
                Paragraph::new(Text::from("未选择邮件"))
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(ratatui::layout::Alignment::Center),
                area,
            );
            return Ok(());
        };

        // 上下分区：邮件头 + 正文
        let chunks: [Rect; 2] = Layout::vertical([
            Constraint::Length(7),                        // 邮件头
            Constraint::Min(1),                           // 正文
        ])
        .areas(area);
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
                format!(" 日  期: {}", mail.date),
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
        // 判断是否有真正可读的纯文本内容（很多 HTML 邮件的 body_text 是空字符串）
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
            // 只有 HTML 正文，提示用户切换视图
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

        Ok(())
    }
}

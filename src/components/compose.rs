use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use tokio::sync::mpsc::UnboundedSender;

use super::Component;
use crate::action::Action;

#[derive(Clone)]
struct ComposeField {
    label: &'static str,
    value: String,
}

#[derive(Default)]
pub struct Compose {
    command_tx: Option<UnboundedSender<Action>>,
    fields: Vec<ComposeField>,
    focus: usize,
    /// 回复模式：填充收件人/主题
    reply_to: Option<String>,
    reply_all: bool,
    forward: bool,
}

impl Compose {
    pub fn new() -> Self {
        Self {
            fields: vec![
                ComposeField { label: "收件人", value: String::new() },
                ComposeField { label: "抄送", value: String::new() },
                ComposeField { label: "密送", value: String::new() },
                ComposeField { label: "主题", value: String::new() },
                ComposeField { label: "正文", value: String::new() },
            ],
            focus: 0,
            ..Default::default()
        }
    }

    /// 回复时预填收件人、主题
    pub fn set_reply(&mut self, from: &str, subject: &str) {
        self.fields[0].value = from.to_string();
        self.fields[3].value = format!("Re: {}", subject.trim_start_matches("Re: ").trim());
        self.reply_to = Some(String::new());
    }

    /// 回复全部时预填收件人+抄送
    pub fn set_reply_all(&mut self, from: &str, cc: &[String], subject: &str) {
        self.fields[0].value = from.to_string();
        if !cc.is_empty() {
            self.fields[1].value = cc.join("; ");
        }
        self.fields[3].value = format!("Re: {}", subject.trim_start_matches("Re: ").trim());
        self.reply_all = true;
    }

    /// 转发时预填主题
    pub fn set_forward(&mut self, subject: &str) {
        self.fields[3].value = format!("Fwd: {}", subject.trim_start_matches("Fwd: ").trim());
        self.forward = true;
    }

    /// 构建待发送邮件（供 App 调用）
    pub fn build_outgoing(&self, from: &str) -> mail_protocol::OutgoingEmail {
        mail_protocol::OutgoingEmail {
            from: from.to_string(),
            to: split_recipients(&self.fields[0].value),
            cc: split_recipients(&self.fields[1].value),
            bcc: split_recipients(&self.fields[2].value),
            subject: self.fields[3].value.clone(),
            body_text: Some(self.fields[4].value.clone()),
            body_html: None,
            attachments: Vec::new(),
            in_reply_to: None,
            references: Vec::new(),
        }
    }

    fn reset(&mut self) {
        for f in &mut self.fields {
            f.value.clear();
        }
        self.focus = 0;
        self.reply_to = None;
        self.reply_all = false;
        self.forward = false;
    }
}

fn split_recipients(s: &str) -> Vec<String> {
    s.split(|c| c == ',' || c == ';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

impl Component for Compose {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<Option<Action>> {
        // Ctrl+S → 发送
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            if self.fields[0].value.is_empty() {
                return Ok(Some(Action::Error("收件人不能为空".into())));
            }
            return Ok(Some(Action::Send));
        }

        match key.code {
            KeyCode::Tab | KeyCode::Down => {
                self.focus = (self.focus + 1) % self.fields.len();
            }
            KeyCode::Up => {
                self.focus = self.focus.saturating_sub(1);
            }
            KeyCode::Enter => {
                // Enter 在正文中换行，在其它字段跳到下一项
                if self.focus == 4 {
                    self.fields[4].value.push('\n');
                } else {
                    self.focus = (self.focus + 1) % self.fields.len();
                }
            }
            KeyCode::Char(c) if self.focus < self.fields.len() => {
                self.fields[self.focus].value.push(c);
            }
            KeyCode::Backspace if self.focus < self.fields.len() => {
                self.fields[self.focus].value.pop();
            }
            KeyCode::Esc => {
                self.reset();
                return Ok(Some(Action::CancelCompose));
            }
            _ => {}
        }
        Ok(None)
    }

    fn update(&mut self, action: Action) -> color_eyre::Result<Option<Action>> {
        match action {
            Action::Reply | Action::ReplyAll | Action::Forward => {
                // 这些由 App 在切换模式时调用 set_* 预填
            }
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        let chunks: [Rect; 3] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .areas(area);

        // 标题
        let title = if self.forward {
            " ➡️ 转发邮件 "
        } else if self.reply_all {
            " ↩️ 回复全部 "
        } else if self.reply_to.is_some() {
            " ↩️ 回复邮件 "
        } else {
            " ✉️ 写新邮件 "
        };
        frame.render_widget(
            Paragraph::new(Text::from(Line::from(Span::styled(
                title,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )))),
            chunks[0],
        );

        // 表单字段列表
        let mut items: Vec<ListItem> = Vec::new();
        let prefix_style = Style::default().fg(Color::Cyan);

        for (i, f) in self.fields.iter().enumerate() {
            let focused = i == self.focus;
            let val_style = if focused {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };

            if i == 4 {
                // 正文区：多行显示，支持换行
                let lines: Vec<Line> = if f.value.is_empty() && !focused {
                    // 无焦点 + 空白：显示占位提示
                    vec![Line::from(vec![
                        Span::styled("  ", prefix_style),
                        Span::styled(" 正文  ", prefix_style),
                        Span::styled(" <输入正文>", val_style),
                    ])]
                } else if f.value.is_empty() && focused {
                    // 有焦点 + 空白：显示标签 + 光标
                    vec![Line::from(vec![
                        Span::styled("▸ ", prefix_style),
                        Span::styled(" 正文  ", prefix_style),
                        Span::styled("|", val_style),
                    ])]
                } else {
                    let mut all_lines: Vec<&str> = f.value.lines().collect();
                    // .lines() 不保留末尾换行后的空行，手动补上
                    if f.value.ends_with('\n') {
                        all_lines.push("");
                    }
                    let total = all_lines.len();
                    let display_lines: &[&str] = if total > 30 { &all_lines[..30] } else { &all_lines };
                    let mut result: Vec<Line> = Vec::with_capacity(display_lines.len() + 1);
                    for (li, line) in display_lines.iter().enumerate() {
                        let is_last = li == display_lines.len() - 1;
                        // 焦点在当前字段时，最后一行末尾加光标
                        let text = if focused && is_last {
                            format!("{}|", line)
                        } else {
                            line.to_string()
                        };
                        if li == 0 {
                            result.push(Line::from(vec![
                                Span::styled(if focused { "▸ " } else { "  " }, prefix_style),
                                Span::styled(" 正文  ", prefix_style),
                                Span::styled(text, val_style),
                            ]));
                        } else {
                            result.push(Line::from(vec![
                                Span::raw("        "),
                                Span::styled(text, val_style),
                            ]));
                        }
                    }
                    if total > 30 {
                        result.push(Line::from(vec![
                            Span::raw("        "),
                            Span::styled(
                                format!("… 共 {} 行", total),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]));
                    }
                    result
                };
                items.push(ListItem::new(Text::from(lines)));
            } else {
                // 普通字段：单行
                let display_val = if f.value.is_empty() && !focused {
                    format!(" <输入{}>", f.label)
                } else {
                    f.value.clone()
                };
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(
                        if focused { "▸ " } else { "  " },
                        prefix_style,
                    ),
                    Span::styled(
                        format!(" {:<6}", f.label),
                        prefix_style,
                    ),
                    Span::styled(display_val, val_style),
                ])));
            }
        }

        let list_mode = if self.forward { " 转发 " } else if self.reply_all { " 回复全部 " } else if self.reply_to.is_some() { " 回复 " } else { " 新邮件 " };
        frame.render_widget(
            List::new(items).block(Block::default().borders(Borders::ALL).title(list_mode)),
            chunks[1],
        );

        // 底部提示
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Tab/↑↓ ", Style::default().fg(Color::DarkGray)),
                Span::raw("切换  "),
                Span::styled(" Enter ", Style::default().fg(Color::DarkGray)),
                Span::raw("换行(正文)  "),
                Span::styled(" Ctrl+S ", Style::default().fg(Color::DarkGray)),
                Span::raw("发送  "),
                Span::styled(" Esc ", Style::default().fg(Color::DarkGray)),
                Span::raw("取消"),
            ])),
            chunks[2],
        );

        Ok(())
    }
}

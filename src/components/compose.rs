use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mail_protocol::AttachmentData;
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

#[derive(Clone, Default)]
struct ComposeField {
    label: &'static str,
    value: String,
}

#[derive(Default)]
pub struct Compose {
    command_tx: Option<UnboundedSender<Action>>,
    fields: Vec<ComposeField>,
    /// 焦点：0..fields.len()-1 是字段，fields.len()-1 是附件区域
    focus: usize,
    /// 附属件列表
    pub(crate) attachments: Vec<AttachmentData>,
    /// 附件区域的选中索引
    att_focus: usize,
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
                ComposeField { label: "附件路径", value: String::new() },
            ],
            focus: 0,
            attachments: Vec::new(),
            att_focus: 0,
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

    /// 添加附件（由 App 读取文件后调用）
    pub fn add_attachment(&mut self, data: AttachmentData) {
        self.attachments.push(data);
        self.fields[5].value.clear();
    }

    /// 移除指定附件
    pub fn remove_attachment(&mut self, idx: usize) {
        if idx < self.attachments.len() {
            self.attachments.remove(idx);
            if self.att_focus >= self.attachments.len() {
                self.att_focus = self.attachments.len().saturating_sub(1);
            }
        }
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
            attachments: self.attachments.clone(),
            in_reply_to: None,
            references: Vec::new(),
        }
    }

    fn total_items(&self) -> usize {
        // 6 fields + optional attachments section
        self.fields.len() + if self.attachments.is_empty() { 0 } else { 1 }
    }

    fn att_section_idx(&self) -> usize {
        self.fields.len() // after all fields
    }

    fn reset(&mut self) {
        for f in &mut self.fields {
            f.value.clear();
        }
        self.focus = 0;
        self.attachments.clear();
        self.att_focus = 0;
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

fn format_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
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

        // 在附件区域导航
        if self.focus == self.att_section_idx() && !self.attachments.is_empty() {
            match key.code {
                KeyCode::Char('d') => {
                    return Ok(Some(Action::RemoveAttachment(self.att_focus)));
                }
                KeyCode::Left => {
                    if self.att_focus > 0 {
                        self.att_focus -= 1;
                    }
                    return Ok(None);
                }
                KeyCode::Right => {
                    if self.att_focus + 1 < self.attachments.len() {
                        self.att_focus += 1;
                    }
                    return Ok(None);
                }
                KeyCode::Down | KeyCode::Tab => {
                    self.focus = 0;
                    return Ok(None);
                }
                KeyCode::Up => {
                    self.focus = self.fields.len().saturating_sub(1);
                    return Ok(None);
                }
                _ => {}
            }
            return Ok(None);
        }

        match key.code {
            KeyCode::Tab | KeyCode::Down => {
                self.focus = (self.focus + 1) % self.total_items();
            }
            KeyCode::Up => {
                if self.focus == 0 {
                    self.focus = self.total_items().saturating_sub(1);
                } else {
                    self.focus -= 1;
                }
            }
            KeyCode::Enter => {
                if self.focus == 5 {
                    // 附件路径字段：回车添加附件
                    let path = self.fields[5].value.trim().to_string();
                    if !path.is_empty() {
                        return Ok(Some(Action::AddAttachment(path)));
                    }
                } else if self.focus == 4 {
                    self.fields[4].value.push('\n');
                } else {
                    self.focus = (self.focus + 1) % self.total_items();
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
            Action::Reply | Action::ReplyAll | Action::Forward => {}
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        let header_h = 2u16;
        let fields_h = Constraint::Min(1);
        let att_h = if self.attachments.is_empty() {
            Constraint::Length(1)
        } else {
            Constraint::Length(2 + self.attachments.len().min(5) as u16)
        };
        let bottom_h = Constraint::Length(2);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(header_h), fields_h, att_h, bottom_h])
            .split(area);

        // ── 标题 ──
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

        // ── 表单字段 ──
        let mut items: Vec<ListItem> = Vec::new();
        let prefix_style = Style::default().fg(Color::Cyan);

        for (i, f) in self.fields.iter().enumerate() {
            let focused = i == self.focus;
            let empty = f.value.is_empty();

            if i == 4 {
                // 正文区：多行显示
                let lines: Vec<Line> = self.build_body_lines(&f.value, focused, empty);
                items.push(ListItem::new(Text::from(lines)));
            } else {
                // 单行字段
                let val_style = if focused {
                    Style::default().fg(Color::White).bg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::White)
                };

                let display_val = if empty && !focused {
                    let hint = match i {
                        0 => " <输入收件人邮箱>",
                        1 => " <抄送，可选>",
                        2 => " <密送，可选>",
                        3 => " <邮件主题>",
                        5 => " <输入文件路径，回车添加附件>",
                        _ => "",
                    };
                    hint.to_string()
                } else {
                    f.value.clone()
                };

                items.push(ListItem::new(Line::from(vec![
                    Span::styled(if focused { "▸ " } else { "  " }, prefix_style),
                    Span::styled(format!(" {:<6}", f.label), prefix_style),
                    Span::styled(display_val, val_style),
                ])));
            }
        }

        let list_mode = if self.forward {
            " 转发 "
        } else if self.reply_all {
            " 回复全部 "
        } else if self.reply_to.is_some() {
            " 回复 "
        } else {
            " 新邮件 "
        };
        frame.render_widget(
            List::new(items).block(Block::default().borders(Borders::ALL).title(list_mode)),
            chunks[1],
        );

        // ── 附件区 ──
        if self.attachments.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " 在上方「附件路径」输入文件路径，按回车添加",
                    Style::default().fg(Color::DarkGray),
                ))),
                chunks[2],
            );
        } else {
            let att_focused = self.focus == self.att_section_idx();
            let att_lines: Vec<Line> = self
                .attachments
                .iter()
                .enumerate()
                .map(|(i, att)| {
                    let selected = i == self.att_focus && att_focused;
                    let prefix = if selected { "▶ " } else { "  " };
                    let size = format_size(att.data.len() as u64);
                    Line::from(Span::styled(
                        format!("{prefix}[{i}] {}  ({size})", att.filename),
                        if selected {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ))
                })
                .collect();
            let hint = Line::from(Span::styled(
                " d 移除选中附件    ←→ 选择附件",
                Style::default().fg(Color::DarkGray),
            ));

            let mut all_lines = att_lines;
            all_lines.push(Line::from(""));
            all_lines.push(hint);

            frame.render_widget(
                Paragraph::new(Text::from(all_lines))
                    .block(Block::default().borders(Borders::ALL).title(" 📎 附件 ")),
                chunks[2],
            );
        }

        // ── 底部提示 ──
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Tab/↑↓ ", Style::default().fg(Color::DarkGray)),
                Span::raw("切换  "),
                Span::styled(" Enter ", Style::default().fg(Color::DarkGray)),
                Span::raw("换行(正文)/添加附件  "),
                Span::styled(" Ctrl+S ", Style::default().fg(Color::DarkGray)),
                Span::raw("发送  "),
                Span::styled(" Esc ", Style::default().fg(Color::DarkGray)),
                Span::raw("取消"),
            ])),
            chunks[3],
        );

        Ok(())
    }
}

impl Compose {
    fn build_body_lines(&self, value: &str, focused: bool, empty: bool) -> Vec<Line<'static>> {
        let prefix_style = Style::default().fg(Color::Cyan);
        let val_style = if focused {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };

        if empty && !focused {
            return vec![Line::from(vec![
                Span::styled("  ", prefix_style),
                Span::styled(" 正文  ", prefix_style),
                Span::styled(" <输入正文>", val_style),
            ])];
        }

        if empty && focused {
            return vec![Line::from(vec![
                Span::styled("▸ ", prefix_style),
                Span::styled(" 正文  ", prefix_style),
                Span::styled("|", val_style),
            ])];
        }

        let mut all_lines: Vec<&str> = value.lines().collect();
        if value.ends_with('\n') {
            all_lines.push("");
        }
        let total = all_lines.len();
        let display_lines: &[&str] = if total > 30 { &all_lines[..30] } else { &all_lines };
        let mut result: Vec<Line> = Vec::with_capacity(display_lines.len() + 1);
        for (li, line) in display_lines.iter().enumerate() {
            let is_last = li == display_lines.len() - 1;
            let text = if focused && is_last {
                format!("{line}|")
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
                Span::styled(format!("… 共 {total} 行"), Style::default().fg(Color::DarkGray)),
            ]));
        }
        result
    }
}

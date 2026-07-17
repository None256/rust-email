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

/// 表单字段定义
#[derive(Clone)]
struct FormField {
    label: &'static str,
    value: String,
    is_password: bool,
}

impl FormField {
    fn new(label: &'static str, is_password: bool) -> Self {
        Self { label, value: String::new(), is_password }
    }

    fn display(&self) -> String {
        if self.is_password && !self.value.is_empty() {
            "•".repeat(self.value.len())
        } else {
            self.value.clone()
        }
    }
}

#[derive(Default)]
pub struct AccountForm {
    command_tx: Option<UnboundedSender<Action>>,
    fields: Vec<FormField>,
    focus: usize,
    security_idx: usize,
    security_options: [&'static str; 3],
}

impl AccountForm {
    pub fn new() -> Self {
        Self {
            fields: vec![
                FormField::new("邮箱", false),
                FormField::new("显示名称", false),
                FormField::new("IMAP 主机", false),
                FormField::new("IMAP 端口", false),
                FormField::new("SMTP 主机", false),
                FormField::new("SMTP 端口", false),
                FormField::new("用户名", false),
                FormField::new("密码", true),
            ],
            security_options: ["TLS (993/465)", "STARTTLS (143/587)", "无加密"],
            ..Default::default()
        }
    }

    /// 获取表单数据（供 App 调用）
    pub fn get_data(&self) -> Option<AccountFormData> {
        let email = self.fields[0].value.clone();
        if email.is_empty() { return None; }
        Some(AccountFormData {
            email,
            display_name: self.fields[1].value.clone(),
            imap_host: if self.fields[2].value.is_empty() { "imap.qq.com".into() } else { self.fields[2].value.clone() },
            imap_port: self.fields[3].value.parse().unwrap_or(993),
            smtp_host: if self.fields[4].value.is_empty() { "smtp.qq.com".into() } else { self.fields[4].value.clone() },
            smtp_port: self.fields[5].value.parse().unwrap_or(465),
            username: if self.fields[6].value.is_empty() { self.fields[0].value.clone() } else { self.fields[6].value.clone() },
            password: self.fields[7].value.clone(),
            security: match self.security_idx { 1 => "start_tls", 2 => "none", _ => "tls" }.into(),
        })
    }

    fn reset(&mut self) {
        for f in &mut self.fields { f.value.clear(); }
        self.focus = 0;
        self.security_idx = 0;
    }
}

/// 账户表单数据
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AccountFormData {
    pub email: String,
    pub display_name: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub security: String,
}

impl Component for AccountForm {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<Option<Action>> {
        // Ctrl+S → 提交
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            if self.fields[0].value.is_empty() {
                return Ok(Some(Action::Error("邮箱不能为空".into())));
            }
            return Ok(Some(Action::SaveAccount));
        }

        match key.code {
            KeyCode::Tab | KeyCode::Down => {
                let total = self.fields.len() + 1; // +1 for security
                self.focus = (self.focus + 1) % total;
            }
            KeyCode::Up => {
                self.focus = self.focus.saturating_sub(1);
            }
            KeyCode::Enter => {
                if self.focus == self.fields.len() {
                    // 切换安全模式
                    self.security_idx = (self.security_idx + 1) % 3;
                } else {
                    let total = self.fields.len() + 1;
                    self.focus = (self.focus + 1) % total;
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
                return Ok(Some(Action::Back));
            }
            _ => {}
        }
        Ok(None)
    }

    fn update(&mut self, _action: Action) -> color_eyre::Result<Option<Action>> {
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        let chunks: [Rect; 3] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(3)])
            .areas(area);

        // 标题
        frame.render_widget(
            Paragraph::new(Text::from(Line::from(Span::styled(
                " ➕ 添加邮箱账户 ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )))),
            chunks[0],
        );

        // 表单字段
        let mut items: Vec<ListItem> = self.fields.iter().enumerate().map(|(i, f)| {
            let focused = i == self.focus;
            let val = f.display();
            let display = if val.is_empty() && !focused {
                format!(" <输入{}>", f.label)
            } else { val };
            ListItem::new(Line::from(vec![
                Span::styled(if focused { "▸ " } else { "  " }, Style::default().fg(Color::Cyan)),
                Span::styled(format!(" {:<12}", f.label), Style::default().fg(Color::Cyan)),
                Span::styled(display, if focused {
                    Style::default().fg(Color::White).bg(Color::DarkGray)
                } else { Style::default().fg(Color::White) }),
            ]))
        }).collect();

        // 安全模式行
        let sec_focused = self.focus == self.fields.len();
        items.push(ListItem::new(Line::from(vec![
            Span::styled(if sec_focused { "▸ " } else { "  " }, Style::default().fg(Color::Cyan)),
            Span::styled(" 安全模式  ", Style::default().fg(Color::Cyan)),
            Span::styled(self.security_options[self.security_idx], if sec_focused {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            } else { Style::default().fg(Color::White) }),
        ])));

        frame.render_widget(
            List::new(items).block(Block::default().borders(Borders::ALL).title(" 账户信息 ")),
            chunks[1],
        );

        // 底部提示
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Tab/↑↓ ", Style::default().fg(Color::DarkGray)),
                Span::raw("切换  "),
                Span::styled(" Enter ", Style::default().fg(Color::DarkGray)),
                Span::raw("确认/下一项  "),
                Span::styled(" Ctrl+S ", Style::default().fg(Color::DarkGray)),
                Span::raw("保存  "),
                Span::styled(" Esc ", Style::default().fg(Color::DarkGray)),
                Span::raw("取消"),
            ])),
            chunks[2],
        );

        Ok(())
    }
}

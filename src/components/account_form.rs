use std::collections::HashMap;

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

/// 常用邮箱提供商配置
struct ProviderInfo {
    imap_host: &'static str,
    imap_port: u16,
    smtp_host: &'static str,
    smtp_port: u16,
    security: &'static str, // "tls" / "start_tls" / "none"
}

fn build_provider_map() -> HashMap<&'static str, ProviderInfo> {
    let mut m: HashMap<&'static str, ProviderInfo> = HashMap::new();
    m.insert("qq.com",       ProviderInfo { imap_host: "imap.qq.com",       imap_port: 993, smtp_host: "smtp.qq.com",       smtp_port: 465, security: "tls" });
    m.insert("foxmail.com",  ProviderInfo { imap_host: "imap.qq.com",       imap_port: 993, smtp_host: "smtp.qq.com",       smtp_port: 465, security: "tls" });
    m.insert("gmail.com",    ProviderInfo { imap_host: "imap.gmail.com",    imap_port: 993, smtp_host: "smtp.gmail.com",    smtp_port: 587, security: "start_tls" });
    m.insert("googlemail.com", ProviderInfo { imap_host: "imap.gmail.com",  imap_port: 993, smtp_host: "smtp.gmail.com",    smtp_port: 587, security: "start_tls" });
    m.insert("outlook.com",  ProviderInfo { imap_host: "outlook.office365.com", imap_port: 993, smtp_host: "smtp.office365.com", smtp_port: 587, security: "start_tls" });
    m.insert("hotmail.com",  ProviderInfo { imap_host: "outlook.office365.com", imap_port: 993, smtp_host: "smtp.office365.com", smtp_port: 587, security: "start_tls" });
    m.insert("live.com",     ProviderInfo { imap_host: "outlook.office365.com", imap_port: 993, smtp_host: "smtp.office365.com", smtp_port: 587, security: "start_tls" });
    m.insert("163.com",      ProviderInfo { imap_host: "imap.163.com",      imap_port: 993, smtp_host: "smtp.163.com",      smtp_port: 465, security: "tls" });
    m.insert("126.com",      ProviderInfo { imap_host: "imap.126.com",      imap_port: 993, smtp_host: "smtp.126.com",      smtp_port: 465, security: "tls" });
    m.insert("yeah.net",     ProviderInfo { imap_host: "imap.yeah.net",     imap_port: 993, smtp_host: "smtp.yeah.net",     smtp_port: 465, security: "tls" });
    m.insert("sina.com",     ProviderInfo { imap_host: "imap.sina.com",     imap_port: 993, smtp_host: "smtp.sina.com",     smtp_port: 465, security: "tls" });
    m.insert("sohu.com",     ProviderInfo { imap_host: "imap.sohu.com",     imap_port: 993, smtp_host: "smtp.sohu.com",     smtp_port: 465, security: "tls" });
    m.insert("aliyun.com",   ProviderInfo { imap_host: "imap.aliyun.com",   imap_port: 993, smtp_host: "smtp.aliyun.com",   smtp_port: 465, security: "tls" });
    m.insert("icloud.com",   ProviderInfo { imap_host: "imap.mail.me.com",  imap_port: 993, smtp_host: "smtp.mail.me.com",  smtp_port: 587, security: "start_tls" });
    m.insert("me.com",       ProviderInfo { imap_host: "imap.mail.me.com",  imap_port: 993, smtp_host: "smtp.mail.me.com",  smtp_port: 587, security: "start_tls" });
    m.insert("yandex.com",   ProviderInfo { imap_host: "imap.yandex.com",   imap_port: 993, smtp_host: "smtp.yandex.com",   smtp_port: 465, security: "tls" });
    m.insert("zoho.com",     ProviderInfo { imap_host: "imap.zoho.com",     imap_port: 993, smtp_host: "smtp.zoho.com",     smtp_port: 465, security: "tls" });
    m.insert("protonmail.com", ProviderInfo { imap_host: "127.0.0.1",       imap_port: 1143, smtp_host: "127.0.0.1",       smtp_port: 1025, security: "none" }); // ProtonMail Bridge
    m
}

/// 表单字段定义
#[derive(Clone)]
struct FormField {
    label: &'static str,
    value: String,
    is_password: bool,
    /// 是否是自动填充的（显示灰色提示）
    auto_filled: bool,
}

impl FormField {
    fn new(label: &'static str, is_password: bool) -> Self {
        Self { label, value: String::new(), is_password, auto_filled: false }
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
    /// 邮箱域名 → 提供商配置
    providers: HashMap<&'static str, ProviderInfo>,
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
            providers: build_provider_map(),
            ..Default::default()
        }
    }

    /// 根据输入的邮箱自动填充 IMAP/SMTP 配置
    fn auto_fill(&mut self) {
        let email = self.fields[0].value.clone();
        let domain = email.split('@').nth(1).unwrap_or("").to_lowercase();
        let info = match self.providers.get(domain.as_str()) {
            Some(p) => p,
            None => return,
        };
        self.fields[2].value = info.imap_host.into();
        self.fields[2].auto_filled = true;
        self.fields[3].value = info.imap_port.to_string();
        self.fields[3].auto_filled = true;
        self.fields[4].value = info.smtp_host.into();
        self.fields[4].auto_filled = true;
        self.fields[5].value = info.smtp_port.to_string();
        self.fields[5].auto_filled = true;
        // 用户名默认填邮箱
        if self.fields[6].value.is_empty() {
            self.fields[6].value = email;
        }
        // 安全模式
        self.security_idx = match info.security {
            "start_tls" => 1,
            "none" => 2,
            _ => 0,
        };
    }

    /// 获取表单数据（供 App 调用）
    pub fn get_data(&self) -> Option<AccountFormData> {
        let email = self.fields[0].value.clone();
        if email.is_empty() { return None; }
        Some(AccountFormData {
            email,
            display_name: self.fields[1].value.clone(),
            imap_host: self.fields[2].value.clone(),
            imap_port: self.fields[3].value.parse().unwrap_or(993),
            smtp_host: self.fields[4].value.clone(),
            smtp_port: self.fields[5].value.parse().unwrap_or(465),
            username: if self.fields[6].value.is_empty() { self.fields[0].value.clone() } else { self.fields[6].value.clone() },
            password: self.fields[7].value.clone(),
            security: match self.security_idx { 1 => "start_tls", 2 => "none", _ => "tls" }.into(),
        })
    }

    fn reset(&mut self) {
        for f in &mut self.fields {
            f.value.clear();
            f.auto_filled = false;
        }
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
                let prev = self.focus;
                let total = self.fields.len() + 1; // +1 for security
                self.focus = (self.focus + 1) % total;
                // 离开邮箱字段时自动填充
                if prev == 0 && !self.fields[0].value.is_empty() {
                    self.auto_fill();
                }
            }
            KeyCode::Up => {
                let prev = self.focus;
                self.focus = self.focus.saturating_sub(1);
                // 离开邮箱字段时自动填充
                if prev == 0 && !self.fields[0].value.is_empty() {
                    self.auto_fill();
                }
            }
            KeyCode::Enter => {
                let prev = self.focus;
                if self.focus == self.fields.len() {
                    // 切换安全模式
                    self.security_idx = (self.security_idx + 1) % 3;
                } else {
                    let total = self.fields.len() + 1;
                    self.focus = (self.focus + 1) % total;
                }
                // 离开邮箱字段时自动填充
                if prev == 0 && !self.fields[0].value.is_empty() {
                    self.auto_fill();
                }
            }
            KeyCode::Char(c) if self.focus < self.fields.len() => {
                self.fields[self.focus].value.push(c);
                // 用户在自动填充字段输入时取消自动标记
                if self.fields[self.focus].auto_filled {
                    self.fields[self.focus].auto_filled = false;
                }
            }
            KeyCode::Backspace if self.focus < self.fields.len() => {
                self.fields[self.focus].value.pop();
                // 用户在自动填充字段输入时取消自动标记
                if self.fields[self.focus].auto_filled {
                    self.fields[self.focus].auto_filled = false;
                }
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
            } else if f.auto_filled && !focused {
                format!("{} (自动)", val)
            } else { val };
            ListItem::new(Line::from(vec![
                Span::styled(if focused { "▸ " } else { "  " }, Style::default().fg(Color::Cyan)),
                Span::styled(format!(" {:<12}", f.label), Style::default().fg(Color::Cyan)),
                Span::styled(display, if focused {
                    Style::default().fg(Color::White).bg(Color::DarkGray)
                } else if f.auto_filled {
                    Style::default().fg(Color::DarkGray)
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

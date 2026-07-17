use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
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
    security: &'static str,
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
    m.insert("protonmail.com", ProviderInfo { imap_host: "127.0.0.1",       imap_port: 1143, smtp_host: "127.0.0.1",       smtp_port: 1025, security: "none" });
    m
}

#[derive(Clone)]
struct FormField {
    label: &'static str,
    /// 未填写时显示的占位提示
    placeholder: &'static str,
    /// 字段下方的说明文字
    hint: &'static str,
    value: String,
    is_password: bool,
    auto_filled: bool,
}

impl FormField {
    fn new(label: &'static str, placeholder: &'static str, hint: &'static str, is_password: bool) -> Self {
        Self { label, placeholder, hint, value: String::new(), is_password, auto_filled: false }
    }

    fn display(&self) -> String {
        if self.is_password && !self.value.is_empty() {
            "•".repeat(self.value.len())
        } else {
            self.value.clone()
        }
    }

    fn placeholder_text(&self) -> &str {
        if self.value.is_empty() { self.placeholder } else { "" }
    }
}

const BASIC_COUNT: usize = 4;
const ADV_START: usize = 4;

#[derive(Default)]
pub struct AccountForm {
    command_tx: Option<UnboundedSender<Action>>,
    fields: Vec<FormField>,
    focus: usize,
    security_idx: usize,
    security_options: [&'static str; 3],
    providers: HashMap<&'static str, ProviderInfo>,
    /// 检测到的提供商域名
    detected_provider: Option<String>,
}

impl AccountForm {
    pub fn new() -> Self {
        Self {
            fields: vec![
                FormField::new("邮箱", "user@example.com", "输入完整邮箱地址，例如 user@163.com", false),
                FormField::new("显示名称", "张三", "发件时显示的名称，可留空", false),
                FormField::new("用户名", "同邮箱", "通常与邮箱地址相同，可留空", false),
                FormField::new("密码", "授权码 / 应用密码", "⚠ 使用授权码而非登录密码（见下方说明）", true),
                FormField::new("IMAP 主机", "imap.example.com", "收件服务器地址，输入邮箱后自动填充", false),
                FormField::new("IMAP 端口", "993", "收件端口：TLS=993 / STARTTLS=143", false),
                FormField::new("SMTP 主机", "smtp.example.com", "发件服务器地址，输入邮箱后自动填充", false),
                FormField::new("SMTP 端口", "465", "发件端口：TLS=465 / STARTTLS=587", false),
            ],
            security_options: ["TLS 加密 (993/465)", "STARTTLS (143/587)", "无加密 (不推荐)"],
            providers: build_provider_map(),
            detected_provider: None,
            ..Default::default()
        }
    }

    fn total_fields(&self) -> usize {
        self.fields.len() + 1
    }

    fn auto_fill(&mut self) {
        let email = self.fields[0].value.clone();
        let domain = email.split('@').nth(1).unwrap_or("").to_lowercase();
        let info = match self.providers.get(domain.as_str()) {
            Some(p) => {
                self.detected_provider = Some(domain);
                p
            }
            None => {
                self.detected_provider = None;
                return;
            }
        };
        self.fields[ADV_START + 0].value = info.imap_host.into();
        self.fields[ADV_START + 0].auto_filled = true;
        self.fields[ADV_START + 1].value = info.imap_port.to_string();
        self.fields[ADV_START + 1].auto_filled = true;
        self.fields[ADV_START + 2].value = info.smtp_host.into();
        self.fields[ADV_START + 2].auto_filled = true;
        self.fields[ADV_START + 3].value = info.smtp_port.to_string();
        self.fields[ADV_START + 3].auto_filled = true;
        if self.fields[2].value.is_empty() {
            self.fields[2].value = email;
        }
        self.security_idx = match info.security {
            "start_tls" => 1,
            "none" => 2,
            _ => 0,
        };
    }

    pub fn get_data(&self) -> Option<AccountFormData> {
        let email = self.fields[0].value.clone();
        if email.is_empty() { return None; }
        Some(AccountFormData {
            email,
            display_name: self.fields[1].value.clone(),
            imap_host: self.fields[ADV_START + 0].value.clone(),
            imap_port: self.fields[ADV_START + 1].value.parse().unwrap_or(993),
            smtp_host: self.fields[ADV_START + 2].value.clone(),
            smtp_port: self.fields[ADV_START + 3].value.parse().unwrap_or(465),
            username: if self.fields[2].value.is_empty() { self.fields[0].value.clone() } else { self.fields[2].value.clone() },
            password: self.fields[3].value.clone(),
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
        self.detected_provider = None;
    }
}

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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            if self.fields[0].value.is_empty() {
                return Ok(Some(Action::Error("邮箱不能为空".into())));
            }
            return Ok(Some(Action::SaveAccount));
        }

        match key.code {
            KeyCode::Tab | KeyCode::Down => {
                let prev = self.focus;
                let total = self.total_fields();
                self.focus = (self.focus + 1) % total;
                if prev == 0 && !self.fields[0].value.is_empty() {
                    self.auto_fill();
                }
            }
            KeyCode::Up => {
                let prev = self.focus;
                self.focus = self.focus.saturating_sub(1);
                if prev == 0 && !self.fields[0].value.is_empty() {
                    self.auto_fill();
                }
            }
            KeyCode::Enter => {
                let prev = self.focus;
                if self.focus == self.fields.len() {
                    self.security_idx = (self.security_idx + 1) % 3;
                } else {
                    let total = self.total_fields();
                    self.focus = (self.focus + 1) % total;
                }
                if prev == 0 && !self.fields[0].value.is_empty() {
                    self.auto_fill();
                }
            }
            KeyCode::Char(c) if self.focus < self.fields.len() => {
                self.fields[self.focus].value.push(c);
                if self.fields[self.focus].auto_filled {
                    self.fields[self.focus].auto_filled = false;
                }
            }
            KeyCode::Backspace if self.focus < self.fields.len() => {
                self.fields[self.focus].value.pop();
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
        let area = inner_rect(area, 1);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),       // 标题
                Constraint::Length(10),      // 基本设置
                Constraint::Length(12),      // 高级设置
                Constraint::Min(4),          // 帮助说明（填满剩余空间）
            ])
            .split(area);

        // ── 标题 ──
        let title = if let Some(ref domain) = self.detected_provider {
            format!(" ➕ 添加邮箱账户 — 已识别: {domain}")
        } else {
            " ➕ 添加邮箱账户 ".into()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                title,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))),
            chunks[0],
        );

        // ── 基本设置 ──
        let basic_items = self.render_fields(0, BASIC_COUNT);
        frame.render_widget(
            List::new(basic_items)
                .block(Block::default().borders(Borders::ALL).title(" 📋 基本设置 "))
                .highlight_style(Style::default()),
            chunks[1],
        );

        // ── 高级设置 ──
        let mut adv_items = self.render_fields(BASIC_COUNT, self.fields.len());

        // 安全模式行
        let sec_focused = self.focus == self.fields.len();
        let sec_desc = match self.security_idx {
            1 => "  发件也走 TLS            ",
            2 => "  明文传输，仅限测试        ",
            _ => "  收发均加密，推荐          ",
        };
        adv_items.push(ListItem::new(vec![
            Line::from(vec![
                Span::styled(if sec_focused { "▸ " } else { "  " }, Style::default().fg(Color::Cyan)),
                Span::styled(" 加密方式  ", Style::default().fg(Color::Cyan)),
                Span::styled(self.security_options[self.security_idx], if sec_focused {
                    Style::default().fg(Color::White).bg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::White)
                }),
            ]),
            Line::from(Span::styled(sec_desc, Style::default().fg(Color::DarkGray))),
        ]));
        frame.render_widget(
            List::new(adv_items)
                .block(Block::default().borders(Borders::ALL).title(" ⚙ 高级设置（输入邮箱后自动填充） ")),
            chunks[2],
        );

        // ── 帮助说明 ──
        let help_text = vec![
            Line::from(Span::styled("💡 如何获取授权码 / 应用密码：", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(vec![
                Span::styled("  163 / 126 / yeah.net  ", Style::default().fg(Color::Cyan)),
                Span::raw("→ 网页登录 → 设置 → POP3/SMTP/IMAP → 开启并获取「授权码」"),
            ]),
            Line::from(vec![
                Span::styled("  QQ / Foxmail           ", Style::default().fg(Color::Cyan)),
                Span::raw("→ 网页登录 → 设置 → 账户 → 生成「授权码」"),
            ]),
            Line::from(vec![
                Span::styled("  Gmail                  ", Style::default().fg(Color::Cyan)),
                Span::raw("→ 开启两步验证 → 生成「应用专用密码」"),
            ]),
            Line::from(vec![
                Span::styled("  Outlook / Hotmail       ", Style::default().fg(Color::Cyan)),
                Span::raw("→ 使用登录密码即可（需开启 IMAP/SMTP）"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(Color::Red)),
                Span::raw("不要直接输入邮箱登录密码。国内邮箱必须先在网页设置中开启 IMAP/SMTP 服务。"),
            ]),
            Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(Color::Red)),
                Span::raw("密码/授权码将以明文存储在本地数据库中，请确保电脑安全。"),
            ]),
        ];
        let help_p = Paragraph::new(Text::from(help_text))
            .block(Block::default().borders(Borders::ALL).title(" 帮助 "))
            .wrap(Wrap { trim: false });
        frame.render_widget(help_p, chunks[3]);

        Ok(())
    }
}

impl AccountForm {
    /// 渲染一行字段：主行 + 提示行
    fn render_fields(&self, start: usize, end: usize) -> Vec<ListItem<'static>> {
        self.fields[start..end]
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let idx = start + i;
                let focused = idx == self.focus;
                let val = f.display();

                // 主行：▸ 标签: 值
                let is_empty = val.is_empty();
                let display_val = if is_empty && !focused {
                    f.placeholder_text().to_string()
                } else {
                    val
                };
                let val_style = if is_empty && !focused {
                    Style::default().fg(Color::DarkGray)
                } else if focused {
                    Style::default().fg(Color::White).bg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::White)
                };
                let main_line = Line::from(vec![
                    Span::styled(if focused { "▸ " } else { "  " }, Style::default().fg(Color::Cyan)),
                    Span::styled(format!(" {:<10}", f.label), Style::default().fg(Color::Cyan)),
                    Span::styled(display_val, val_style),
                ]);

                // 提示行：灰色说明文字
                let hint_text = if focused && f.auto_filled {
                    "已自动填充，可按需修改"
                } else if focused {
                    f.hint
                } else if f.auto_filled {
                    "已自动填充"
                } else if !f.value.is_empty() {
                    ""
                } else {
                    f.hint
                };
                let hint_line = if hint_text.is_empty() {
                    Line::from("")
                } else {
                    Line::from(Span::styled(
                        format!("              {}", hint_text),
                        Style::default().fg(Color::DarkGray),
                    ))
                };

                ListItem::new(vec![main_line, hint_line])
            })
            .collect()
    }
}

/// 给区域加内边距（上下左右各缩进 margin 行/列）
fn inner_rect(area: Rect, margin: u16) -> Rect {
    Rect {
        x: area.x + margin,
        y: area.y + margin,
        width: area.width.saturating_sub(margin * 2),
        height: area.height.saturating_sub(margin * 2),
    }
}

use crossterm::event::{KeyCode, KeyEvent};
use mail_protocol::AccountConfig;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use tokio::sync::mpsc::UnboundedSender;

use super::Component;
use crate::{action::Action, config::Config};

/// 首页 — 显示账户列表
#[derive(Default)]
pub struct Home {
    command_tx: Option<UnboundedSender<Action>>,
    config: Config,
    pub accounts: Vec<AccountConfig>,
    pub state: ListState,
}

impl Home {
    pub fn new() -> Self {
        Self {
            state: ListState::default().with_selected(Some(0)),
            ..Self::default()
        }
    }

    pub fn set_accounts(&mut self, accounts: Vec<AccountConfig>) {
        let is_empty = accounts.is_empty();
        self.accounts = accounts;
        self.state.select(if is_empty { None } else { Some(0) });
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }

    fn next(&mut self) {
        let i = self.state.selected()
            .map(|i| (i + 1).min(self.accounts.len().saturating_sub(1)))
            .unwrap_or(0);
        self.state.select(Some(i));
    }

    fn prev(&mut self) {
        let i = self.state.selected()
            .map(|i| i.saturating_sub(1))
            .unwrap_or(0);
        self.state.select(Some(i));
    }
}

impl Component for Home {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn register_config_handler(&mut self, config: Config) -> color_eyre::Result<()> {
        self.config = config;
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<Option<Action>> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.next(),
            KeyCode::Char('k') | KeyCode::Up => self.prev(),
            KeyCode::Char('a') => return Ok(Some(Action::AddAccount)),
            KeyCode::Char('d') if !self.accounts.is_empty() => {
                return Ok(Some(Action::DeleteAccount));
            }
            _ => {}
        }
        Ok(None)
    }

    fn update(&mut self, _action: Action) -> color_eyre::Result<Option<Action>> {
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        let chunks: [Rect; 2] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);

        // 标题
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(Span::styled(" 📧 我的邮箱 ",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::styled(
                    format!("共 {} 个账户", self.accounts.len()),
                    Style::default().fg(Color::DarkGray),
                )),
            ])),
            chunks[0],
        );

        // 列表
        if self.accounts.is_empty() {
            frame.render_widget(
                Paragraph::new(Text::from(vec![
                    Line::from(Span::styled("还没有添加邮箱账户", Style::default().fg(Color::DarkGray))),
                    Line::from(Span::styled("按 a 添加新账户", Style::default().fg(Color::Cyan))),
                ])).alignment(Alignment::Center),
                chunks[1],
            );
        } else {
            let items: Vec<ListItem> = self.accounts.iter().map(|a| {
                let name = if a.username.contains('@') { &a.username } else { &a.imap_host };
                ListItem::new(Line::from(vec![
                    Span::styled(" 📬 ", Style::default().fg(Color::Yellow)),
                    Span::styled(format!(" {:<30}", name), Style::default().fg(Color::White)),
                    Span::styled(format!(" ({})", a.imap_host), Style::default().fg(Color::DarkGray)),
                ]))
            }).collect();

            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::default().borders(Borders::ALL).title(" 选择账户，Enter 连接 "))
                    .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
                    .highlight_symbol("▸ "),
                chunks[1], &mut self.state,
            );
        }

        Ok(())
    }
}

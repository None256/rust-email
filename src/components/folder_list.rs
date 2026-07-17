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
pub struct FolderList {
    command_tx: Option<UnboundedSender<Action>>,
    pub folders: Vec<mail_protocol::Folder>,
    pub state: ListState,
}

impl FolderList {
    pub fn new() -> Self {
        Self {
            state: ListState::default().with_selected(Some(0)),
            ..Self::default()
        }
    }

    pub fn set_folders(&mut self, folders: Vec<mail_protocol::Folder>) {
        self.folders = folders;
        self.state.select(if self.folders.is_empty() {
            None
        } else {
            Some(0)
        });
    }

    pub fn selected_folder(&self) -> Option<&mail_protocol::Folder> {
        self.state
            .selected()
            .and_then(|i| self.folders.get(i))
    }

    fn next(&mut self) {
        let i = self
            .state
            .selected()
            .map(|i| (i + 1).min(self.folders.len().saturating_sub(1)))
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

impl Component for FolderList {
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
                if let Some(folder) = self.selected_folder() {
                    return Ok(Some(Action::SelectFolder(folder.name.clone())));
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn update(&mut self, action: Action) -> color_eyre::Result<Option<Action>> {
        match action {
            Action::Tick => {}
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 📂 文件夹 ")
            .style(Style::default().fg(Color::Cyan));
        let inner = block.inner(area);

        let items: Vec<ListItem> = self
            .folders
            .iter()
            .map(|f| {
                let display_name = folder_display_name(&f.name);
                ListItem::new(Line::from(Span::raw(format!(" {} ", display_name))))
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

        // 空状态提示
        if self.folders.is_empty() {
            frame.render_widget(
                Paragraph::new(Text::from("暂无文件夹\n按 q 返回"))
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(ratatui::layout::Alignment::Center),
                inner,
            );
        }

        Ok(())
    }
}

/// 文件夹名显示：UTF-7 解码 → 中文翻译（不修改原始名称，原始名称用于 IMAP 命令）
pub fn folder_display_name(name: &str) -> String {
    crate::utils::folder_display_name(name)
}

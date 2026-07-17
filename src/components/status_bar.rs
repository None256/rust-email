use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use tokio::sync::mpsc::UnboundedSender;

use super::Component;
use crate::{action::Action, app::Mode};

/// 底部状态栏 — 显示连接状态、当前文件夹、模式快捷键提示、临时消息
#[derive(Default)]
pub struct StatusBar {
    command_tx: Option<UnboundedSender<Action>>,
    connected: bool,
    mode: Mode,
    current_folder: Option<String>,
    status_message: Option<(String, Instant)>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// 由 App 在模式切换时调用
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// 清除当前文件夹显示（回到首页时调用）
    pub fn clear_folder(&mut self) {
        self.current_folder = None;
    }

    fn set_message(&mut self, msg: String) {
        self.status_message = Some((msg, Instant::now()));
    }
}

impl Component for StatusBar {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn update(&mut self, action: Action) -> color_eyre::Result<Option<Action>> {
        match action {
            Action::Connected => self.connected = true,
            Action::Disconnect => {
                self.connected = false;
                self.current_folder = None;
            }
            Action::ConnectionFailed(msg) => self.set_message(msg),
            Action::SelectFolder(name) => self.current_folder = Some(name),
            Action::Error(msg) => self.set_message(msg),
            Action::Send => self.set_message("✓ 邮件已发送".into()),
            Action::DeleteMail => self.set_message("✓ 邮件已删除".into()),
            Action::ToggleFlag => self.set_message("✓ 已切换星标".into()),
            Action::ToggleRead => self.set_message("✓ 已切换已读/未读".into()),
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        // 每次绘制前检查临时消息是否超时
        if let Some((_, time)) = self.status_message {
            if time.elapsed().as_secs() > 3 {
                self.status_message = None;
            }
        }
        // 只取底部 1 行
        let areas: [Rect; 2] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        let bottom = areas[1];

        // 分隔线：在状态栏上方画一条水平线
        let separator_y = bottom.y.saturating_sub(1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::raw("─".repeat(bottom.width as usize))))
                .style(Style::default().fg(Color::DarkGray)),
            Rect::new(bottom.x, separator_y, bottom.width, 1),
        );

        // 左右分区（固定宽度，避免截断）
        let right_width = right_hint_width();
        let left_width = bottom.width.saturating_sub(right_width);
        let cols: [Rect; 2] = Layout::horizontal([
            Constraint::Length(left_width),
            Constraint::Length(right_width),
        ])
        .areas(bottom);
        let (left_area, right_area) = (cols[0], cols[1]);

        // ── 左侧：连接状态 + 文件夹 ──
        let (icon, color) = if self.connected {
            ("●", Color::Green)
        } else {
            ("○", Color::Red)
        };
        let status = if self.connected { "已连接" } else { "未连接" };
        let folder = self
            .current_folder
            .as_deref()
            .map(|n| {
                let decoded = utf7_imap::decode_utf7_imap(n.to_string());
                translate_folder(&decoded)
            })
            .unwrap_or_else(|| "未选择文件夹".to_string());

        let left_line = Line::from(vec![
            Span::styled(format!(" {icon} "), Style::default().fg(color)),
            Span::styled(status, Style::default().fg(color)),
            Span::raw(" | "),
            Span::raw(folder),
        ]);
        frame.render_widget(Paragraph::new(left_line), left_area);

        // ── 右侧：临时消息 or 快捷键提示 ──
        let right_line = if let Some((ref msg, _)) = self.status_message {
            Line::from(Span::styled(
                format!(" ⓘ {msg} "),
                Style::default().fg(Color::Yellow),
            ))
        } else {
            let hint = match self.mode {
                Mode::Home => "q:退出  ↑↓:选择  Enter:连接  a:添加  d:删除",
                Mode::AccountForm => "Tab/↑↓:切换  Enter:确认  Ctrl+S:保存  Esc:取消",
                Mode::FolderList => "q:退出  j↓/k↑  Enter:进入  R:刷新",
                Mode::MailList => "q:退出  j↓/k↑  Enter:查看  r:回复  d:删除  s:星标",
                Mode::MailView => "q:退出  r:回复  d:删除  h:HTML  t:文本  j/k:上下  s:星标",
                Mode::Compose => "Ctrl+S:发送  Esc:取消",
            };
            Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))
        };
        frame.render_widget(Paragraph::new(right_line).right_aligned(), right_area);

        Ok(())
    }
}

/// 右侧提示文字需要的宽度（取最长的提示）
fn right_hint_width() -> u16 {
    50 // 足够容纳最长的快捷键提示
}

/// 文件夹名翻译
fn translate_folder(name: &str) -> String {
    match name {
        "INBOX" => "收件箱".into(),
        "Sent" | "Sent Messages" | "Sent Items" => "已发送".into(),
        "Drafts" => "草稿箱".into(),
        "Trash" | "Deleted Messages" | "Deleted Items" => "垃圾箱".into(),
        "Junk" | "Spam" | "Junk Email" => "垃圾邮件".into(),
        "Archive" | "Archives" => "归档".into(),
        "Outbox" => "发件箱".into(),
        "Important" => "重要邮件".into(),
        "Flagged" | "Starred" => "星标邮件".into(),
        _ => name.to_string(),
    }
}

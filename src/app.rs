use crossterm::event::KeyEvent;
use mail_protocol::{MailBackend, MailClient};
use ratatui::prelude::Rect;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use mail_protocol::AccountConfig;

use crate::{
    action::Action,
    components::{
        account_form::AccountForm, compose::Compose, folder_list::FolderList, fps::FpsCounter,
        home::Home, mail_list::MailList, mail_view::MailView, status_bar::StatusBar, Component,
    },
    config::Config,
    database::{self, Account, NewAccount},
    tui::{Event, Tui},
};

pub struct App {
    config: Config,
    tick_rate: f64,
    frame_rate: f64,

    // 组件
    home: Home,
    account_form: AccountForm,
    compose: Compose,
    folder_list: FolderList,
    mail_list: MailList,
    mail_view: MailView,
    fps_counter: FpsCounter,
    status_bar: StatusBar,

    // 状态
    should_quit: bool,
    should_suspend: bool,
    mode: Mode,
    connecting: bool,

    // 内存账户管理
    accounts: Vec<Account>,
    active_account_id: Option<i64>,
    database: SqlitePool,

    // 邮件客户端
    mail_client: MailClient,

    // 事件系统
    last_tick_key_events: Vec<KeyEvent>,
    action_tx: mpsc::UnboundedSender<Action>,
    action_rx: mpsc::UnboundedReceiver<Action>,
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    /// 首页 / 账户列表
    #[default]
    Home,
    /// 添加账户表单
    AccountForm,
    /// 文件夹列表
    FolderList,
    /// 邮件列表
    MailList,
    /// 邮件阅读
    MailView,
    /// 写邮件
    Compose,
}

impl App {
    pub async fn new(
        tick_rate: f64,
        frame_rate: f64,
        database: SqlitePool,
    ) -> color_eyre::Result<Self> {
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        let accounts = database::list_accounts(&database).await?;

        let mut app = Self {
            tick_rate,
            frame_rate,
            home: Home::new(),
            account_form: AccountForm::new(),
            compose: Compose::new(),
            folder_list: FolderList::new(),
            mail_list: MailList::new(),
            mail_view: MailView::new(),
            fps_counter: FpsCounter::default(),
            status_bar: StatusBar::new(),
            should_quit: false,
            should_suspend: false,
            config: Config::new()?,
            mode: Mode::Home,
            connecting: false,
            accounts,
            active_account_id: None,
            database,
            mail_client: MailClient::new(),
            last_tick_key_events: Vec::new(),
            action_tx,
            action_rx,
        };
        app.home.set_accounts(app.accounts.clone());
        Ok(app)
    }

    pub async fn run(&mut self) -> color_eyre::Result<()> {
        let mut tui = Tui::new()?
            .tick_rate(self.tick_rate)
            .frame_rate(self.frame_rate);
        tui.enter()?;

        self.status_bar
            .register_action_handler(self.action_tx.clone())?;
        self.status_bar
            .register_config_handler(self.config.clone())?;
        self.status_bar.init(tui.size()?)?;
        self.status_bar.set_mode(self.mode);

        let action_tx = self.action_tx.clone();
        loop {
            self.handle_events(&mut tui).await?;
            self.handle_actions(&mut tui).await?;
            if self.should_suspend {
                tui.suspend()?;
                action_tx.send(Action::Resume)?;
                action_tx.send(Action::ClearScreen)?;
                tui.enter()?;
            } else if self.should_quit {
                tui.stop()?;
                break;
            }
        }
        tui.exit()?;
        Ok(())
    }

    /// 返回当前 mode 对应的主组件
    fn active_component(&mut self) -> &mut dyn Component {
        match self.mode {
            Mode::Home => &mut self.home,
            Mode::AccountForm => &mut self.account_form,
            Mode::FolderList => &mut self.folder_list,
            Mode::MailList => &mut self.mail_list,
            Mode::MailView => &mut self.mail_view,
            Mode::Compose => &mut self.compose,
        }
    }

    async fn handle_events(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        let Some(event) = tui.next_event().await else {
            return Ok(());
        };
        let action_tx = self.action_tx.clone();
        match event {
            Event::Quit => action_tx.send(Action::Quit)?,
            Event::Tick => action_tx.send(Action::Tick)?,
            Event::Render => action_tx.send(Action::Render)?,
            Event::Resize(x, y) => action_tx.send(Action::Resize(x, y))?,
            Event::Key(key) => self.handle_key_event(key)?,
            _ => {}
        }
        // 只将事件分发给当前 mode 的组件
        if let Some(action) = self.active_component().handle_events(Some(event.clone()))? {
            action_tx.send(action)?;
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<()> {
        // 文本输入模式下由组件完全接管按键处理
        if matches!(self.mode, Mode::AccountForm | Mode::Compose) {
            return Ok(());
        }
        let action_tx = self.action_tx.clone();
        let Some(keymap) = self.config.keybindings.0.get(&self.mode) else {
            return Ok(());
        };
        match keymap.get(&vec![key]) {
            Some(action) => {
                info!("Got action: {action:?}");
                action_tx.send(action.clone())?;
            }
            _ => {
                self.last_tick_key_events.push(key);
                if let Some(action) = keymap.get(&self.last_tick_key_events) {
                    info!("Got action: {action:?}");
                    action_tx.send(action.clone())?;
                }
            }
        }
        Ok(())
    }

    async fn handle_actions(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        while let Ok(action) = self.action_rx.try_recv() {
            if action != Action::Tick && action != Action::Render {
                debug!("{action:?}");
            }
            match &action {
                // ── 系统动作 ──
                Action::Tick => {
                    self.last_tick_key_events.drain(..);
                    if self.mail_client.is_connected() {
                        let _ = self.mail_client.noop().await;
                    }
                }
                Action::Quit => self.should_quit = true,
                Action::Suspend => self.should_suspend = true,
                Action::Resume => self.should_suspend = false,
                Action::ClearScreen => {
                    let _ = tui.terminal.clear();
                }
                Action::Resize(_, _) => {}
                Action::Render => {
                    self.render(tui)?;
                    // 更新组件
                    self.fps_counter.update(Action::Render)?;
                }

                // ── 账户管理 ──
                Action::AddAccount => {
                    self.switch_mode(Mode::AccountForm);
                }
                Action::SaveAccount => {
                    if let Some(data) = self.account_form.get_data() {
                        let config = AccountConfig {
                            imap_host: data.imap_host,
                            imap_port: data.imap_port,
                            smtp_host: data.smtp_host,
                            smtp_port: data.smtp_port,
                            username: data.username,
                            password: data.password,
                            security: match data.security.as_str() {
                                "start_tls" => mail_protocol::SecurityMode::StartTls,
                                "none" => mail_protocol::SecurityMode::None,
                                _ => mail_protocol::SecurityMode::Tls,
                            },
                        };
                        let account = NewAccount {
                            email: data.email,
                            display_name: (!data.display_name.is_empty())
                                .then_some(data.display_name),
                            config,
                        };
                        database::save_account(&self.database, &account).await?;
                        self.accounts = database::list_accounts(&self.database).await?;
                        self.home.set_accounts(self.accounts.clone());
                        info!("已添加账户");
                    }
                    self.switch_mode(Mode::Home);
                }
                Action::DeleteAccount => {
                    if let Some(i) = self.home.selected_index() {
                        if i < self.accounts.len() {
                            database::delete_account(&self.database, self.accounts[i].id).await?;
                            self.accounts.remove(i);
                            self.home.set_accounts(self.accounts.clone());
                            info!("已删除账户");
                        }
                    }
                }

                // ── 写邮件 ──
                Action::Compose => {
                    self.compose = Compose::new();
                    self.switch_mode(Mode::Compose);
                }
                Action::Reply => {
                    if let Some(ref mail) = self.mail_view.mail {
                        self.compose = Compose::new();
                        self.compose.set_reply(&mail.from, &mail.subject);
                        self.switch_mode(Mode::Compose);
                    }
                }
                Action::ReplyAll => {
                    if let Some(ref mail) = self.mail_view.mail {
                        self.compose = Compose::new();
                        self.compose.set_reply_all(&mail.from, &mail.to, &mail.subject);
                        self.switch_mode(Mode::Compose);
                    }
                }
                Action::Forward => {
                    if let Some(ref mail) = self.mail_view.mail {
                        self.compose = Compose::new();
                        self.compose.set_forward(&mail.subject);
                        self.switch_mode(Mode::Compose);
                    }
                }
                Action::Send => {
                    if let Some(account) = self
                        .active_account_id
                        .and_then(|id| self.accounts.iter().find(|a| a.id == id))
                    {
                        let email = self.compose.build_outgoing(&account.config.username);
                        match mail_protocol::smtp::SmtpSender::send(&account.config, &email).await {
                            Ok(_) => {
                                info!("邮件发送成功");
                                self.switch_mode(
                                    if self.mail_list.current_folder.is_empty() {
                                        Mode::Home
                                    } else {
                                        Mode::MailList
                                    },
                                );
                            }
                            Err(e) => {
                                error!("SMTP 发送失败: {e}");
                                self.action_tx.send(Action::Error(format!("发送失败: {e}")))?;
                            }
                        }
                    } else {
                        self.action_tx
                            .send(Action::Error("未连接账户，无法发送".into()))?;
                    }
                }
                Action::CancelCompose => {
                    self.switch_mode(Mode::MailList);
                }

                // ── 连接管理 ──
                Action::Connect if !self.connecting => {
                    self.connecting = true;
                    let config = match self
                        .home
                        .selected_index()
                        .and_then(|i| self.accounts.get(i))
                    {
                        Some(account) => {
                            self.active_account_id = Some(account.id);
                            account.config.clone()
                        }
                        None => {
                            self.connecting = false;
                            self.action_tx
                                .send(Action::ConnectionFailed("请先添加一个账户".into()))?;
                            break;
                        }
                    };
                    match self.mail_client.connect(&config).await {
                        Ok(_) => {
                            info!("IMAP 连接成功");
                            self.connecting = false;
                            self.action_tx.send(Action::Connected)?;
                        }
                        Err(e) => {
                            self.connecting = false;
                            self.action_tx
                                .send(Action::ConnectionFailed(format!("连接失败: {e}")))?;
                        }
                    }
                }
                Action::Connected => {
                    info!("已连接，获取文件夹列表...");
                    match self.mail_client.list_folders().await {
                        Ok(folders) => {
                            info!("获取到 {} 个文件夹", folders.len());
                            if let Some(account_id) = self.active_account_id {
                                database::cache_folders(&self.database, account_id, &folders)
                                    .await?;
                            }
                            self.folder_list.set_folders(folders);
                            self.switch_mode(Mode::FolderList);
                        }
                        Err(e) => {
                            if let Some(account_id) = self.active_account_id {
                                let folders =
                                    database::cached_folders(&self.database, account_id).await?;
                                if !folders.is_empty() {
                                    self.folder_list.set_folders(folders);
                                    self.switch_mode(Mode::FolderList);
                                    continue;
                                }
                            }
                            self.action_tx
                                .send(Action::Error(format!("获取文件夹失败: {e}")))?;
                        }
                    }
                }
                Action::ConnectionFailed(_) => {}
                Action::Disconnect => {
                    let _ = self.mail_client.disconnect().await;
                    self.switch_mode(Mode::Home);
                }

                // ── 文件夹 ──
                Action::SelectFolder(name) => {
                    self.mail_list.current_folder = name.clone();
                    self.switch_mode(Mode::MailList);
                    info!("获取文件夹 {} 的邮件...", name);
                    match self.mail_client.fetch_latest_messages(name, 50).await {
                        Ok(mails) => {
                            info!("获取到 {} 封邮件", mails.len());
                            if let Some(account_id) = self.active_account_id {
                                database::cache_email_summaries(
                                    &self.database,
                                    account_id,
                                    name,
                                    &mails,
                                )
                                .await?;
                            }
                            self.mail_list.set_mails(mails);
                        }
                        Err(e) => {
                            if let Some(account_id) = self.active_account_id {
                                let mails = database::cached_email_summaries(
                                    &self.database,
                                    account_id,
                                    name,
                                    50,
                                )
                                .await?;
                                if !mails.is_empty() {
                                    self.mail_list.set_mails(mails);
                                    continue;
                                }
                            }
                            self.action_tx
                                .send(Action::Error(format!("获取 {name} 邮件列表失败: {e}")))?;
                        }
                    }
                }
                Action::RefreshFolders => match self.mail_client.list_folders().await {
                    Ok(folders) => {
                        self.folder_list.set_folders(folders);
                    }
                    Err(e) => {
                        self.action_tx
                            .send(Action::Error(format!("刷新文件夹失败: {e}")))?;
                    }
                },

                // ── 邮件列表 ──
                Action::LoadMails => {}
                Action::ViewMail => {
                    if let Some(uid) = self.mail_list.selected_uid() {
                        self.switch_mode(Mode::MailView);
                        let folder = self.mail_list.current_folder.clone();
                        info!("获取邮件 UID={} 来自 {}", uid, folder);
                        match self.mail_client.fetch_message(&folder, uid).await {
                            Ok(mail) => {
                                // 标记为已读
                                let _ = self
                                    .mail_client
                                    .add_flags(&folder, &[uid], &[mail_protocol::MailFlag::Seen])
                                    .await;
                                // 更新本地标记
                                if let Some(i) = self.mail_list.state.selected() {
                                    if let Some(m) = self.mail_list.mails.get_mut(i) {
                                        if !m.flags.contains(&mail_protocol::MailFlag::Seen) {
                                            m.flags.push(mail_protocol::MailFlag::Seen);
                                        }
                                    }
                                }
                                if let Some(account_id) = self.active_account_id {
                                    database::cache_email(&self.database, account_id, &mail)
                                        .await?;
                                }
                                self.mail_view.set_mail(mail);
                            }
                            Err(e) => {
                                let cached = if let Some(account_id) = self.active_account_id {
                                    database::cached_email(&self.database, account_id, &folder, uid)
                                        .await?
                                } else {
                                    None
                                };
                                if let Some(mail) = cached {
                                    self.mail_view.set_mail(mail);
                                } else {
                                    self.action_tx
                                        .send(Action::Error(format!("查看 {folder}/{uid} 失败: {e}")))?;
                                    self.switch_mode(Mode::MailList);
                                }
                            }
                        }
                    }
                }
                Action::DeleteMail => {
                    let (folder, uid) = if self.mode == Mode::MailList {
                        let uid = self.mail_list.remove_selected();
                        (self.mail_list.current_folder.clone(), uid)
                    } else if self.mode == Mode::MailView {
                        if let Some(ref mail) = self.mail_view.mail {
                            let uid = Some(mail.uid);
                            let folder = mail.folder.clone();
                            self.switch_mode(Mode::MailList);
                            (folder, uid)
                        } else {
                            return Ok(());
                        }
                    } else {
                        return Ok(());
                    };
                    if let Some(uid) = uid {
                        let _ = self
                            .mail_client
                            .add_flags(&folder, &[uid], &[mail_protocol::MailFlag::Deleted])
                            .await;
                        self.action_tx
                            .send(Action::Error("邮件已标记为删除".into()))?;
                    }
                }
                Action::LoadMoreMails => {}
                Action::NextMail | Action::PrevMail => {}
                Action::ToggleFlag => {
                    if self.mode == Mode::MailList {
                        if let Some((uid, flagged)) = self.mail_list.toggle_flag() {
                            let folder = self.mail_list.current_folder.clone();
                            let flag = mail_protocol::MailFlag::Flagged;
                            let _ = if flagged {
                                self.mail_client.add_flags(&folder, &[uid], &[flag]).await
                            } else {
                                self.mail_client.remove_flags(&folder, &[uid], &[flag]).await
                            };
                        }
                    } else if self.mode == Mode::MailView {
                        if let Some(ref mail) = self.mail_view.mail {
                            let folder = mail.folder.clone();
                            let uid = mail.uid;
                            let flagged = mail.flags.contains(&mail_protocol::MailFlag::Flagged);
                            let flag = mail_protocol::MailFlag::Flagged;
                            let _ = if flagged {
                                self.mail_client.remove_flags(&folder, &[uid], &[flag]).await
                            } else {
                                self.mail_client.add_flags(&folder, &[uid], &[flag]).await
                            };
                        }
                    }
                }

                // ── 保存 HTML 正文 ──
                Action::SaveHtml => {
                    if let Some(ref mail) = self.mail_view.mail {
                        if let Some(html) = &mail.body_html {
                            let subject = mail.subject.trim();
                            let safe_name = if subject.is_empty() {
                                format!("email_{}", mail.uid)
                            } else {
                                // 替换文件名中不允许的字符
                                subject
                                    .chars()
                                    .map(|c| if ":<>/\\|?*\"".contains(c) { '_' } else { c })
                                    .collect::<String>()
                            };
                            let desktop = std::env::var("USERPROFILE")
                                .map(|p| std::path::PathBuf::from(p).join("Desktop"))
                                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
                            let save_dir = desktop.join("rust-email-attachments");
                            tokio::fs::create_dir_all(&save_dir).await?;
                            let file_path = save_dir.join(format!("{}.html", &safe_name));
                            // 构建完整的 HTML 文档
                            let full_html = format!(
                                "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">\n<title>{}</title>\n<style>body {{ font-family: sans-serif; padding: 20px; }}</style>\n</head><body>\n{}\n</body></html>",
                                mail.subject, html
                            );
                            tokio::fs::write(&file_path, full_html.as_bytes()).await?;
                            let msg = format!("✓ HTML 已保存: {}", file_path.display());
                            info!("{msg}");
                            self.action_tx.send(Action::Error(msg))?;
                        }
                    }
                }
                // ── 导航 ──
                Action::Back => {
                    let prev = match self.mode {
                        Mode::AccountForm => Mode::Home,
                        Mode::MailView => Mode::MailList,
                        Mode::MailList => Mode::FolderList,
                        Mode::FolderList => Mode::Home,
                        Mode::Compose => Mode::MailList,
                        _ => Mode::Home,
                    };
                    self.switch_mode(prev);
                }

                _ => {}
            }

            // 将 action 转发给 StatusBar 更新状态
            if action != Action::Tick && action != Action::Render {
                self.status_bar.update(action.clone())?;
            }
        }

        Ok(())
    }

    fn switch_mode(&mut self, new_mode: Mode) {
        if new_mode == Mode::Home {
            self.status_bar.clear_folder();
        }
        self.mode = new_mode;
        self.status_bar.set_mode(new_mode);
        info!("切换到模式: {:?}", new_mode);
    }

    #[allow(dead_code)]
    fn handle_resize(&mut self, tui: &mut Tui, w: u16, h: u16) -> color_eyre::Result<()> {
        tui.resize(Rect::new(0, 0, w, h))?;
        self.render(tui)?;
        Ok(())
    }

    fn render(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        tui.draw(|frame| {
            // 只渲染当前 mode 对应的主组件
            if let Err(err) = self.active_component().draw(frame, frame.area()) {
                let _ = self
                    .action_tx
                    .send(Action::Error(format!("Failed to draw: {:?}", err)));
            }
            // StatusBar 最后绘制
            if let Err(err) = self.status_bar.draw(frame, frame.area()) {
                let _ = self
                    .action_tx
                    .send(Action::Error(format!("StatusBar draw error: {:?}", err)));
            }
        })?;
        Ok(())
    }
}

use serde::{Deserialize, Serialize};
use strum::Display;

#[derive(Debug, Clone, PartialEq, Eq, Display, Serialize, Deserialize)]
pub enum Action {
    // ── 系统动作 ──
    Tick,
    Render,
    Resize(u16, u16),
    Suspend,
    Resume,
    Quit,
    ClearScreen,
    Error(String),
    Help,

    // ── 账户管理 ──
    /// 添加新账户（切换到表单）
    AddAccount,
    /// 保存新账户
    SaveAccount,
    /// 删除选中账户
    DeleteAccount,

    // ── 连接管理 ──
    // 连接到邮箱账户
    Connect,
    // 断开连接
    Disconnect,
    // 连接成功
    Connected,
    // 连接失败
    ConnectionFailed(String),

    // ── 文件夹 ──
    // 选中某个文件夹
    SelectFolder(String),
    // 刷新文件夹列表
    RefreshFolders,

    // ── 邮件列表 ──
    // 加载当前文件夹的邮件
    LoadMails,
    // 选中某封邮件 (UID)
    SelectMail(u32),
    // 加载更多邮件 (分页)
    LoadMoreMails,
    // 下一条邮件
    NextMail,
    // 上一条邮件
    PrevMail,

    // ── 邮件查看 ──
    // 查看当前选中的邮件 (UID 从组件状态取)
    ViewMail,
    // 切换 HTML 视图
    ShowHtml,
    // 切换纯文本视图
    ShowText,

    // ── 邮件操作 ──
    // 删除邮件
    DeleteMail,
    // 切换星标
    ToggleFlag,
    // 切换已读/未读
    ToggleRead,
    // 移动到指定文件夹
    MoveMail(String),

    // ── 写邮件 ──
    // 新建邮件
    Compose,
    // 回复
    Reply,
    // 回复全部
    ReplyAll,
    // 转发
    Forward,
    // 发送邮件
    Send,
    // 取消编辑
    CancelCompose,

    // ── 保存正文 ──
    /// 将 HTML 正文保存为文件
    SaveHtml,

    // ── 导航 ──
    // 返回上一级
    Back,
}

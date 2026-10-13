//! 界面能调用的命令.
//!
//! 慢操作 (找回被撤回的消息) 走 `spawn_blocking`, 免得卡住界面线程。

use tauri::{AppHandle, State};

use crate::settings::Settings;
use crate::state::{self, AppState, ChatDto, MessageDto};
use crate::tray;

/// 应用与数据概况.
#[derive(Debug, serde::Serialize)]
pub struct AppInfo {
    /// 版本号.
    pub version: String,
    /// 数据目录.
    pub data_dir: String,
    /// 账号目录.
    pub account: Option<String>,
    /// 当前消息库.
    pub database: Option<String>,
    /// 密钥数量.
    pub keys: usize,
    /// 联系人数量.
    pub contacts: usize,
    /// 归档目录.
    pub archive_dir: String,
    /// 归档快照数量.
    pub archive_snapshots: usize,
    /// 最新快照时间.
    pub archive_latest: Option<i64>,
    /// 设置.
    pub settings: Settings,
}

/// 概况.
#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let (snapshots, latest) = state.archive_summary();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        data_dir: state.data_dir.display().to_string(),
        account: state
            .account
            .as_deref()
            .map(|path| path.display().to_string()),
        database: state
            .database
            .as_deref()
            .map(|path| path.display().to_string()),
        keys: state.keys.len(),
        contacts: state.contacts.len(),
        archive_dir: state.archive_dir.display().to_string(),
        archive_snapshots: snapshots,
        archive_latest: latest,
        settings: state.settings(),
    }
}

/// 会话列表.
#[tauri::command]
pub fn list_chats(state: State<'_, AppState>) -> Result<Vec<ChatDto>, String> {
    state::chats(&state)
}

/// 某个会话的消息.
#[tauri::command]
pub async fn list_messages(
    state: State<'_, AppState>,
    chat: String,
    limit: usize,
) -> Result<Vec<MessageDto>, String> {
    state::messages(&state, &chat, limit.max(1))
}

/// 找回某个会话里被撤回的消息; 这一步要逐页比对, 因此会慢一些.
#[tauri::command]
pub async fn find_revoked(
    state: State<'_, AppState>,
    chat: String,
) -> Result<Vec<MessageDto>, String> {
    state::revoked(&state, &chat)
}

/// 记住设置.
#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) {
    tracing::info!(
        start_hidden = settings.start_hidden,
        hide_dock = settings.hide_dock_when_hidden,
        "设置已更新"
    );
    state.update_settings(settings);
}

/// 显示主窗口.
#[tauri::command]
pub fn show_window(app: AppHandle) {
    tray::show_main_window(&app);
}

/// 退出 (走统一退出路径).
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    tray::quit(&app);
}

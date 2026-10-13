//! 托盘, 窗口显隐与统一退出路径.
//!
//! 三个入口都汇到这里: 托盘菜单, 窗口关闭按钮, 终端 Ctrl+C。全应用只有一个退出实现。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::App;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

/// 最近一次隐藏窗口的时间 (毫秒).
///
/// macOS 在窗口隐藏后会紧接着补一个 activate 事件, 如果不理会它, 窗口会立刻又被唤起。
static LAST_HIDE_MS: AtomicU64 = AtomicU64::new(0);

/// 主窗口标签.
pub const MAIN_WINDOW: &str = "main";

/// 建托盘.
pub fn create<R: Runtime>(app: &App<R>) -> tauri::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let title = MenuItem::with_id(
        app,
        "title",
        format!("wxmsg v{version}"),
        false,
        None::<&str>,
    )?;
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &title,
            &PredefinedMenuItem::separator(app)?,
            &show_item,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    let handle = app.handle().clone();
    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon().cloned().ok_or_else(|| {
                tauri::Error::AssetNotFound("缺少应用图标, 无法建立托盘".to_owned())
            })?,
        )
        // tooltip 第一行放应用名。
        .tooltip(format!("wxmsg v{version}\n微信本地聊天记录管理"))
        // 左键走显示窗口, 右键才弹菜单。
        .show_menu_on_left_click(false)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                tracing::info!("托盘菜单: 显示主窗口");
                show_main_window(app);
            }
            "quit" => {
                tracing::info!("托盘菜单: 退出");
                quit(app);
            }
            other => tracing::debug!(id = other, "托盘菜单项未处理"),
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                tracing::info!("左键点击托盘图标: 显示主窗口");
                show_main_window(tray.app_handle());
            }
        })
        .build(&handle)?;
    Ok(())
}

/// 显示并聚焦主窗口; 已经显示时就聚焦, 不新建窗口.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        tracing::warn!("找不到主窗口");
        return;
    };
    // 先把应用身份切回普通应用, dock 图标随之出现, 再显示窗口。
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Regular) {
            tracing::warn!(%error, "恢复 dock 图标失败");
        }
    }
    if let Err(error) = window.unminimize() {
        tracing::debug!(%error, "取消最小化失败");
    }
    if let Err(error) = window.show() {
        tracing::warn!(%error, "显示窗口失败");
    }
    if let Err(error) = window.set_focus() {
        tracing::debug!(%error, "聚焦窗口失败");
    }
}

/// 隐藏主窗口; 是否同时隐藏 dock 图标由设置决定.
pub fn hide_main_window<R: Runtime>(app: &AppHandle<R>, hide_dock: bool) {
    LAST_HIDE_MS.store(now_ms(), Ordering::Relaxed);
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    if let Err(error) = window.hide() {
        tracing::warn!(%error, "隐藏窗口失败");
    }
    #[cfg(target_os = "macos")]
    if hide_dock && let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
        tracing::warn!(%error, "隐藏 dock 图标失败");
    }
}

/// 隐藏后紧接着到来的 activate 事件是否应当忽略.
pub fn should_ignore_activate() -> bool {
    let last = LAST_HIDE_MS.load(Ordering::Relaxed);
    last != 0 && now_ms().saturating_sub(last) < 800
}

/// 统一退出路径: 记日志, 然后退出.
///
/// 日志的 guard 由 `run` 持有, 进程结束时才落盘, 因此这里不需要手动 flush。
pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    tracing::info!("收到退出请求, 正在收尾");
    app.exit(0);
}

/// 主窗口关闭按钮的行为: 隐藏而不是退出.
pub fn on_close_requested<R: Runtime>(window: &tauri::Window<R>) {
    let app = window.app_handle().clone();
    let hide_dock = app
        .try_state::<crate::state::AppState>()
        .map(|state| state.settings().hide_dock_when_hidden)
        .unwrap_or(true);
    tracing::info!(hide_dock, "窗口请求关闭: 隐藏到托盘");
    hide_main_window(&app, hide_dock);
}

/// 当前毫秒时间戳.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

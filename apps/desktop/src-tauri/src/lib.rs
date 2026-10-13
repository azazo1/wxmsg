//! wxmsg 的桌面界面.
//!
//! 界面只做三件事: 看会话与消息, 看归档状态, 找回被撤回的消息。脏活都在 `wxmsg-core` 里,
//! 与命令行共用同一份实现。
//!
//! 窗口行为按桌面应用的习惯来: 关闭按钮只是隐藏, 真正退出走托盘菜单或 Ctrl+C。

pub const BUNDLE_ID: &str = "com.azazo1.wxmsg";

mod commands;
mod logging;
mod settings;
mod state;
mod tray;

use std::path::PathBuf;

use tauri::Manager;

/// 启动界面.
pub fn run() {
    let data_dir = data_directory();
    // guard 必须活到进程结束, 否则最后一段日志会丢。
    let _log_guard = logging::init(&data_dir);
    let state = state::AppState::load(data_dir.clone());
    let start_hidden = state.settings().start_hidden;

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            tracing::info!(?args, ?cwd, "已有实例在运行, 显示它的主窗口");
            tray::show_main_window(app);
        }))
        // 只记尺寸, 不记位置: 换显示器或分辨率之后, 旧坐标可能落在屏幕外。
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(tauri_plugin_window_state::StateFlags::SIZE)
                .build(),
        )
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_chats,
            commands::list_messages,
            commands::find_revoked,
            commands::save_settings,
            commands::show_window,
            commands::quit_app,
        ])
        .setup(move |app| {
            tray::create(app)?;
            if start_hidden {
                tracing::info!("按设置以隐藏状态启动");
                if let Some(window) = app.get_webview_window(tray::MAIN_WINDOW) {
                    let _ = window.hide();
                }
                #[cfg(target_os = "macos")]
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            install_signal_handler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 关闭按钮不等于退出: 拦截默认行为, 改成隐藏。
                api.prevent_close();
                tray::on_close_requested(window);
            }
        });

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(error) => {
            tracing::error!(%error, "界面构建失败");
            eprintln!("界面构建失败: {error}");
            return;
        }
    };

    app.run(|handle, event| {
        // macOS 上点 dock 图标或 Finder 再次打开会走这里。
        if let tauri::RunEvent::Reopen { .. } = event {
            if tray::should_ignore_activate() {
                // 窗口刚被隐藏, 系统马上补发的这个事件要忽略, 否则窗口会立刻又弹回来。
                tracing::debug!("忽略隐藏后紧接到来的 activate");
            } else {
                tracing::info!("应用被重新打开: 显示主窗口");
                tray::show_main_window(handle);
            }
        }
    });
}

/// 数据目录: 环境变量优先, 否则用平台约定位置.
///
/// `just debug` 通过 `APP_DATA_DIR` 把调试实例的数据与日志都隔离到项目里。
fn data_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("APP_DATA_DIR") {
        return PathBuf::from(path);
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home)
            .join("Library/Application Support")
            .join(BUNDLE_ID),
        None => PathBuf::from(".").join(BUNDLE_ID),
    }
}

/// 终端里 Ctrl+C 也要优雅退出, 而不是被信号直接打死.
///
/// 退出走的是与托盘菜单同一个入口 (`app.exit`), 因此后台服务与日志收尾只有一处实现。
#[cfg(unix)]
fn install_signal_handler(app: tauri::AppHandle) {
    if let Err(error) = ctrlc::set_handler(move || {
        tracing::info!("收到 Ctrl+C, 走统一退出路径");
        app.exit(0);
    }) {
        tracing::warn!(%error, "注册 Ctrl+C 处理器失败");
    }
}

/// 非 unix 平台没有这个信号.
#[cfg(not(unix))]
fn install_signal_handler(_app: tauri::AppHandle) {}

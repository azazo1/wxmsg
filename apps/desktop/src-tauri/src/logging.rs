//! 日志.
//!
//! 默认写到平台日志位置 `~/Library/Logs/<bundle id>/`, 可以用环境变量覆盖成项目内的隔离目录
//! (见 `just debug`)。按天轮转, 启动时清掉过期的文件。

use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// 日志保留天数.
const KEEP_DAYS: u64 = 7;

/// 日志文件名前缀.
const PREFIX: &str = "wxmsg.log";

/// 初始化日志, 返回的 guard 必须在程序结束前一直持有, 否则最后一段日志会丢.
pub fn init(data_dir: &Path) -> Option<WorkerGuard> {
    let directory = log_directory(data_dir);
    if let Err(error) = std::fs::create_dir_all(&directory) {
        // 日志目录建不出来时退化成只写终端, 不影响主流程。
        eprintln!("日志目录创建失败 ({}): {error}", directory.display());
        return None;
    }
    prune(&directory);

    let appender = tracing_appender::rolling::daily(&directory, PREFIX);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("wxmsg_desktop_lib=info,wxmsg_core=info,warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(writer)
        .with_ansi(false)
        .init();

    install_panic_hook();
    tracing::info!(
        directory = %directory.display(),
        version = env!("CARGO_PKG_VERSION"),
        "日志已启动"
    );
    Some(guard)
}

/// 日志目录: 环境变量优先, 否则用平台日志位置.
///
/// 两个环境变量都认: `APP_LOG_DIR` 直接给目录, `APP_LOG_FILE` 给文件路径 (取其所在目录),
/// 后者的写法在项目脚本里更常见。
pub fn log_directory(data_dir: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("APP_LOG_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("APP_LOG_FILE") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            return parent.to_path_buf();
        }
    }
    // macOS 的日志约定位置与数据目录不在一处, 因此这里单独拼一次。
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home)
            .join("Library/Logs")
            .join(crate::BUNDLE_ID),
        None => data_dir.join("logs"),
    }
}

/// 清掉过期的日志文件.
fn prune(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(KEEP_DAYS * 86_400));
    let Some(cutoff) = cutoff else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(PREFIX) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified < cutoff && std::fs::remove_file(entry.path()).is_ok() {
            eprintln!("已删除过期日志 {}", entry.path().display());
        }
    }
}

/// 把 panic 也写进日志, 否则崩了之后只有终端里一闪而过的信息.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "未知位置".to_owned());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "无法读取 panic 内容".to_owned());
        tracing::error!(location = %location, payload = %payload, "程序 panic");
        previous(info);
    }));
}

//! 子命令实现.
//!
//! 每个子命令一个模块, 公共的装配逻辑 (加载密钥, 复制库, 打开连接) 放在本模块。
//! 命令行参数的定义在 [`crate::cli`], 这里只负责执行。

pub mod archive;
pub mod config;
pub mod keep;
pub mod messages;
pub mod plain;
pub mod recalls;
pub mod sessions;
pub mod snap;
mod table;

use anyhow::Context;
use wxmsg_core::config::Config;
use wxmsg_core::{EncryptedStore, KeyStore};

use crate::cli::CommonArgs;

/// 把 Unix 秒格式化为本地时间; 无值时返回 `-`.
///
/// 界面上的一切时间都以本机时区显示, 而 JSON 输出同时给出原始秒值, 便于脚本处理。
pub(crate) fn format_time(unix: Option<i64>) -> String {
    match unix.and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0)) {
        Some(time) => time
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => "-".to_owned(),
    }
}

/// 解析带 K / M / G 后缀的容量写法 (1024 进制; 不带后缀就是字节).
pub(crate) fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, multiplier) = match text.chars().last()?.to_ascii_uppercase() {
        'K' => (&text[..text.len() - 1], 1024u64),
        'M' => (&text[..text.len() - 1], 1024 * 1024),
        'G' => (&text[..text.len() - 1], 1024 * 1024 * 1024),
        'T' => (&text[..text.len() - 1], 1024u64.pow(4)),
        _ => (text, 1),
    };
    let value: f64 = digits.trim().parse().ok()?;
    if value < 0.0 {
        return None;
    }
    Some((value * multiplier as f64) as u64)
}

/// 把字节数写成人能读的形式.
pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("T", 1024 * 1024 * 1024 * 1024),
        ("G", 1024 * 1024 * 1024),
        ("M", 1024 * 1024),
        ("K", 1024),
    ];
    for (unit, scale) in UNITS {
        if bytes >= scale {
            return format!("{:.1}{unit}", bytes as f64 / scale as f64);
        }
    }
    format!("{bytes}B")
}

/// 加载密钥文件.
///
/// 路径来源顺序: `--keys` > 配置文件 > `secrets/all_keys.json`。
/// 密钥内容只由本程序读取, 不进入日志。
pub(crate) fn load_keys(common: &CommonArgs) -> anyhow::Result<KeyStore> {
    let path = match common.keys.clone() {
        Some(path) => path,
        None => load_config(common.config.as_deref())?
            .keys_file()
            .unwrap_or_else(crate::paths::default_keys),
    };
    let keys =
        KeyStore::load(&path).with_context(|| format!("加载密钥文件失败: {}", path.display()))?;
    tracing::info!(count = keys.len(), path = %path.display(), "已加载密钥文件");
    Ok(keys)
}

/// 决定用哪个配置文件.
pub(crate) fn config_path(
    explicit: Option<&std::path::Path>,
) -> anyhow::Result<std::path::PathBuf> {
    match explicit {
        Some(path) => Ok(path.to_path_buf()),
        None => Config::default_path().context("读不到 HOME, 无法确定配置文件位置"),
    }
}

/// 读取配置; 文件不存在时返回内置默认值.
pub(crate) fn load_config(explicit: Option<&std::path::Path>) -> anyhow::Result<Config> {
    let path = config_path(explicit)?;
    Ok(Config::load(&path)?)
}

/// 决定要读取哪一个库文件: 指定 `--copy-to` 时先复制, 否则直接用原路径.
pub(crate) fn prepare_path(common: &CommonArgs) -> anyhow::Result<std::path::PathBuf> {
    let db = common.db.clone().context("需要 --db 指定库文件")?;
    match common.copy_to.as_deref() {
        Some(dir) => {
            let copied = wxmsg_core::copy_database(&db, dir)
                .with_context(|| format!("复制库失败: {}", db.display()))?;
            tracing::info!(
                source = %db.display(),
                copy = %copied.display(),
                "已复制库副本, 后续只读写副本"
            );
            Ok(copied)
        }
        None => {
            tracing::warn!("未指定 --copy-to, 直接打开原始库; 微信正在写入时结果可能不一致");
            Ok(db)
        }
    }
}

/// 按命令行参数准备一个已能读取的库连接.
///
/// 顺序是: 加载密钥 -> 可选地复制库副本 -> 用密钥打开。
pub(crate) fn open_store(common: &CommonArgs) -> anyhow::Result<EncryptedStore> {
    let keys = load_keys(common)?;
    let path = prepare_path(common)?;

    let store = EncryptedStore::open_with_keys(&path, &keys)
        .with_context(|| format!("打开库失败: {}", path.display()))?;
    tracing::info!(
        path = %path.display(),
        tables = store.tables()?.len(),
        "已打开加密库"
    );
    Ok(store)
}

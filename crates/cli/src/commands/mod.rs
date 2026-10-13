//! 子命令实现.
//!
//! 每个子命令一个模块, 公共的装配逻辑 (加载密钥, 复制库, 打开连接) 放在本模块。
//! 命令行参数的定义在 [`crate::cli`], 这里只负责执行。

pub mod messages;
pub mod plain;
pub mod recalls;
pub mod sessions;
mod table;

use anyhow::Context;
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

/// 加载密钥文件.
///
/// 密钥内容只由本程序读取, 不进入日志。
pub(crate) fn load_keys(common: &CommonArgs) -> anyhow::Result<KeyStore> {
    let keys = KeyStore::load(&common.keys)
        .with_context(|| format!("加载密钥文件失败: {}", common.keys.display()))?;
    tracing::info!(
        count = keys.len(),
        path = %common.keys.display(),
        "已加载密钥文件"
    );
    Ok(keys)
}

/// 决定要读取哪一个库文件: 指定 `--copy-to` 时先复制, 否则直接用原路径.
pub(crate) fn prepare_path(common: &CommonArgs) -> anyhow::Result<std::path::PathBuf> {
    match common.copy_to.as_deref() {
        Some(dir) => {
            let copied = wxmsg_core::copy_database(&common.db, dir)
                .with_context(|| format!("复制库失败: {}", common.db.display()))?;
            tracing::info!(
                source = %common.db.display(),
                copy = %copied.display(),
                "已复制库副本, 后续只读写副本"
            );
            Ok(copied)
        }
        None => {
            tracing::warn!("未指定 --copy-to, 直接打开原始库; 微信正在写入时结果可能不一致");
            Ok(common.db.clone())
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

//! `export` 子命令: 把会话导出成 Markdown, JSON 或 CSV.
//!
//! 导出走的是"先读库, 再写文件"的普通路径; 媒体文件按 [`wxmsg_core::media`] 里的规律去
//! 账号目录里找, 找到就复制一份到导出目录的 `media/` 下。加 `--with-revoked` 时顺带跑一次
//! 找回, 把被撤回的消息按时间插回原处并标上 `[撤回]`。

use std::path::{Path, PathBuf};

use anyhow::Context;
use wxmsg_core::contact::ContactBook;
use wxmsg_core::crypto::CipherParams;
use wxmsg_core::export::{self, ExportOptions, Format};
use wxmsg_core::recall;
use wxmsg_core::wal::Wal;

use crate::cli::ExportArgs;
use crate::commands::{load_config, load_keys, prepare_path};
use crate::paths;

pub fn run(args: ExportArgs) -> anyhow::Result<()> {
    let format = Format::parse(&args.format)
        .with_context(|| format!("不认识的格式 {}; 可用 md / json / csv", args.format))?;

    let keys = load_keys(&args.common)?;
    let db_path = prepare_path(&args.common)?;
    let store = wxmsg_core::EncryptedStore::open_with_keys(&db_path, &keys)
        .with_context(|| format!("打开库失败: {}", db_path.display()))?;

    // 账号目录: 先看命令行, 再看配置, 最后从**原始**库的路径往上推。
    //
    // 注意不能用 prepare_path 的结果: 给了 --copy-to 时那是副本, 副本上面没有 db_storage。
    let config = load_config(args.common.config.as_deref())?;
    let account = match args.source.clone() {
        Some(dir) => Some(paths::normalize_source(&dir)),
        None => match config.source_dir() {
            Some(dir) => Some(paths::normalize_source(&dir)),
            None => args
                .common
                .db
                .as_deref()
                .and_then(account_of)
                .or_else(|| account_of(&db_path)),
        },
    };
    if account.is_none() {
        tracing::warn!("认不出账号目录, 这次不导出媒体文件; 可以用 --source 指定");
    }

    let contacts = match account.as_deref() {
        Some(account) => match wxmsg_core::contact::find_database(account, "contact/contact.db") {
            Ok(path) => match ContactBook::load(&path, &keys) {
                Ok(book) => book,
                Err(error) => {
                    tracing::warn!("读联系人失败, 只用 wxid 显示: {error}");
                    ContactBook::default()
                }
            },
            Err(error) => {
                tracing::warn!("找不到联系人库, 只用 wxid 显示: {error}");
                ContactBook::default()
            }
        },
        None => ContactBook::default(),
    };

    let revoked = if args.with_revoked {
        tracing::info!("正在找回被撤回的消息, 这一步需要逐个页版本比对");
        recover(&args, &db_path, &keys)?
    } else {
        Vec::new()
    };

    let options = ExportOptions {
        format,
        output: args.out.clone(),
        account: if args.no_media { None } else { account },
        include_revoked: args.with_revoked,
    };

    let report = export::export(&store, &contacts, &options, &args.chats, &revoked)?;
    println!(
        "导出完成: {} 个会话, {} 条消息{}; 媒体文件 {} 个; 输出目录 {}",
        report.chats,
        report.messages,
        if report.revoked > 0 {
            format!(" (含 {} 条撤回记录)", report.revoked)
        } else {
            String::new()
        },
        report.media,
        args.out.display(),
    );
    Ok(())
}

/// 跑一次找回, 拿到被删的消息.
fn recover(
    args: &ExportArgs,
    db_path: &Path,
    keys: &wxmsg_core::KeyStore,
) -> anyhow::Result<Vec<wxmsg_core::recall::RecoveredMessage>> {
    let key = *keys
        .require(&db_path.display().to_string())
        .with_context(|| format!("找不到 {} 对应的密钥", db_path.display()))?;
    let store = wxmsg_core::EncryptedStore::open_with_keys(db_path, keys)?;

    let mut wal_path = db_path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal_path = PathBuf::from(wal_path);
    let wal = Wal::open_if_present(&wal_path)
        .with_context(|| format!("解析 WAL 失败: {}", wal_path.display()))?;

    let mut messages = Vec::new();
    for chat in &args.chats {
        let report = recall::recover(
            db_path,
            wal.as_ref(),
            key,
            CipherParams::wechat(),
            &store,
            Some(chat),
        )?;
        tracing::info!(
            chat = %chat,
            found = report.messages.len(),
            historical_rows = report.historical_rows,
            "找回结束"
        );
        messages.extend(report.messages);
    }
    if args.chats.is_empty() {
        tracing::info!("没有指定会话, 跳过撤回找回; 加上 --chat 才能带出撤回记录");
    }
    Ok(messages)
}

/// 从库的路径往上找账号目录 (即含 `db_storage` 的那一层).
fn account_of(db_path: &Path) -> Option<PathBuf> {
    for ancestor in db_path.ancestors() {
        if ancestor
            .file_name()
            .is_some_and(|name| name == "db_storage")
        {
            return ancestor.parent().map(Path::to_path_buf);
        }
    }
    None
}

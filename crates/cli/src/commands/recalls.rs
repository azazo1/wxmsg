//! `recalls` 子命令: 从 WAL 里找回被撤回 (被删除) 的消息.
//!
//! 这里不依赖任何事前归档: 只要被删消息所在页的旧版本还在 WAL 文件里就能找回。
//! WAL 是固定大小的环形文件, 因此**越早的撤回越可能已经被覆盖**。

use anyhow::Context;
use serde_json::json;
use wxmsg_core::EncryptedStore;
use wxmsg_core::crypto::CipherParams;
use wxmsg_core::recall::{self, FoundIn, RecoveredMessage};
use wxmsg_core::wal::Wal;

use crate::cli::RecallsArgs;
use crate::commands::{format_time, load_keys, prepare_path, table};

/// 表格里正文列的最大显示宽度.
const TEXT_WIDTH: usize = 80;

pub fn run(args: RecallsArgs) -> anyhow::Result<()> {
    let keys = load_keys(&args.common)?;
    let path = prepare_path(&args.common)?;
    let key = *keys
        .require(&path.display().to_string())
        .with_context(|| format!("找不到 {} 对应的密钥", path.display()))?;
    let store = EncryptedStore::open_with_keys(&path, &keys)?;

    let wal_path = args
        .wal
        .clone()
        .unwrap_or_else(|| with_suffix(&path, "-wal"));
    let wal = Wal::open_if_present(&wal_path)
        .with_context(|| format!("解析 WAL 失败: {}", wal_path.display()))?;
    if let Some(wal) = wal.as_ref() {
        tracing::info!(
            frames = wal.len(),
            commits = wal.commits().len(),
            path = %wal_path.display(),
            "已解析 WAL"
        );
    } else {
        tracing::warn!(path = %wal_path.display(), "没有 WAL 文件, 只能看到当前状态");
    }

    let report = recall::recover(
        &path,
        wal.as_ref(),
        key,
        CipherParams::wechat(),
        &store,
        args.chat.as_deref(),
    )?;
    tracing::info!(
        tables = report.tables,
        pages = report.pages,
        historical_rows = report.historical_rows,
        truncated_rows = report.truncated_rows,
        found = report.messages.len(),
        "找回结束"
    );

    let mut messages: Vec<&RecoveredMessage> = report.messages.iter().collect();
    if messages.is_empty() {
        println!("没有找到可还原的被删消息");
        println!("可能是: WAL 里已没有对应的旧页版本 (环形文件被覆盖), 或者撤回发生在别的时间片");
        return Ok(());
    }
    if args.limit > 0 && messages.len() > args.limit {
        tracing::info!(
            total = messages.len(),
            limit = args.limit,
            "只显示最近的若干条"
        );
        let keep = messages.len() - args.limit;
        messages.drain(..keep);
    }

    if args.common.json {
        let mut payload = Vec::with_capacity(messages.len());
        for item in &messages {
            payload.push(json!({
                "table": item.table,
                "local_id": item.message.local_id,
                "create_time": item.message.create_time,
                "create_time_at": format_time(Some(item.message.create_time)),
                "real_type": item.message.real_type(),
                "sender": item.message.sender()?,
                "text": item.message.text()?,
                "truncated": item.truncated,
                "found_in": match item.found_in {
                    FoundIn::DbFile => "db_file".to_owned(),
                    FoundIn::Wal { generation, frame } => format!("wal#{generation}/frame{frame}"),
                },
                "notice": item.notice.as_ref().map(|notice| json!({
                    "local_id": notice.local_id,
                    "create_time": notice.create_time,
                    "create_time_at": format_time(Some(notice.create_time)),
                    "text": notice.text().unwrap_or_default(),
                })),
            }));
        }
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let rows: Vec<Vec<String>> = messages.iter().map(|item| describe(item)).collect();
    println!(
        "{}",
        table::render(
            &["行号", "时间", "类型", "来源", "撤回通知", "正文"],
            &[true, false, true, false, false, false],
            &rows,
        )
    );
    Ok(())
}

/// 一行的显示内容.
fn describe(item: &RecoveredMessage) -> Vec<String> {
    let mut text = item.message.text().unwrap_or_default();
    text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if item.truncated {
        text.push_str(" [载荷溢出, 内容可能不完整]");
    }

    let notice = match item.notice.as_ref() {
        Some(notice) => format!(
            "{} 行号 {}",
            format_time(Some(notice.create_time)),
            notice.local_id
        ),
        None => "-".to_owned(),
    };
    let origin = match item.found_in {
        FoundIn::DbFile => "主库".to_owned(),
        FoundIn::Wal { generation, frame } => format!("WAL 第 {generation} 代 帧 {frame}"),
    };

    vec![
        item.message.local_id.to_string(),
        format_time(Some(item.message.create_time)),
        item.message.real_type().to_string(),
        origin,
        notice,
        table::truncate(&text, TEXT_WIDTH),
    ]
}

/// 给路径加上后缀, 例如由 `message_1.db` 得到 `message_1.db-wal`.
fn with_suffix(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

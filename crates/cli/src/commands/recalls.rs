//! `recalls` 子命令: 找回被撤回 (被删除) 的消息.
//!
//! 两个来源:
//!
//! - **实时库**: 直接看当前库与 WAL。不需要任何事前准备, 但 WAL 是环形文件, 越早的撤回越可能
//!   已经被覆盖
//! - **归档**: 指定 `--archive` 时, 先把快照里的库与 WAL 还原到临时目录再看。归档是持续留存
//!   的密文, 因此**留多久就能回溯多久**

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::json;
use wxmsg_core::EncryptedStore;
use wxmsg_core::archive::Archive;
use wxmsg_core::crypto::{CipherParams, KEY_SIZE};
use wxmsg_core::recall::{self, FoundIn, RecoveredMessage};
use wxmsg_core::wal::Wal;

use crate::cli::RecallsArgs;
use crate::commands::{format_size, format_time, load_keys, prepare_path, table};

/// 表格里正文列的最大显示宽度.
const TEXT_WIDTH: usize = 80;

/// 还原归档时使用的临时目录 (相对当前目录).
const RESTORE_DIR: &str = ".tmp/recall-restore";

pub fn run(args: RecallsArgs) -> anyhow::Result<()> {
    let mut found = match args.archive.as_deref() {
        Some(root) => from_archive(&args, root)?,
        None => from_live(&args)?,
    };
    found.sort_by_key(|item| item.item.message.create_time);

    if found.is_empty() {
        println!("没有找到可还原的被删消息");
        println!("可能是: 对应的旧页版本已不在 WAL 里 (环形文件被覆盖), 或者该时间段没有归档");
        return Ok(());
    }
    if args.limit > 0 && found.len() > args.limit {
        let keep = found.len() - args.limit;
        tracing::info!(
            total = found.len(),
            limit = args.limit,
            "只显示最近的若干条"
        );
        found.drain(..keep);
    }

    if args.common.json {
        let payload: Vec<_> = found
            .iter()
            .map(|found| {
                Ok(json!({
                    "source": found.label,
                    "table": found.item.table,
                    "local_id": found.item.message.local_id,
                    "create_time": found.item.message.create_time,
                    "create_time_at": format_time(Some(found.item.message.create_time)),
                    "real_type": found.item.message.real_type(),
                    "sender": found.item.message.sender()?,
                    "text": found.item.message.text()?,
                    "truncated": found.item.truncated,
                    "found_in": describe_origin(found.item.found_in),
                    "notice": found.item.notice.as_ref().map(|notice| json!({
                        "local_id": notice.local_id,
                        "create_time": notice.create_time,
                        "create_time_at": format_time(Some(notice.create_time)),
                        "text": notice.text().unwrap_or_default(),
                    })),
                }))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let rows: Vec<Vec<String>> = found.iter().map(describe).collect();
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

/// 一条找回记录连同它的出处.
struct Found {
    /// 来自哪个库 (归档快照路径或实时库路径).
    label: String,
    /// 找回的消息.
    item: RecoveredMessage,
}

/// 一个待检查的库.
struct SourceDb {
    label: String,
    db_path: PathBuf,
    wal_path: PathBuf,
    key: [u8; KEY_SIZE],
    store: EncryptedStore,
}

impl SourceDb {
    /// 跑一次找回.
    fn recover(&self, chat: Option<&str>) -> anyhow::Result<Vec<RecoveredMessage>> {
        let wal = Wal::open_if_present(&self.wal_path)
            .with_context(|| format!("解析 WAL 失败: {}", self.wal_path.display()))?;
        match wal.as_ref() {
            Some(wal) => tracing::info!(
                frames = wal.len(),
                generations = wal.generations.len(),
                path = %self.wal_path.display(),
                "已解析 WAL"
            ),
            None => tracing::warn!(path = %self.wal_path.display(), "没有 WAL, 只能看到当前状态"),
        }

        let report = recall::recover(
            &self.db_path,
            wal.as_ref(),
            self.key,
            CipherParams::wechat(),
            &self.store,
            chat,
        )?;
        tracing::info!(
            db = %self.label,
            tables = report.tables,
            pages = report.pages,
            historical_rows = report.historical_rows,
            truncated_rows = report.truncated_rows,
            found = report.messages.len(),
            "找回结束"
        );
        Ok(report.messages)
    }
}

/// 直接检查实时库.
fn from_live(args: &RecallsArgs) -> anyhow::Result<Vec<Found>> {
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

    let source = SourceDb {
        label: path.display().to_string(),
        db_path: path,
        wal_path,
        key,
        store,
    };
    let label = source.label.clone();
    let messages = source.recover(args.chat.as_deref())?;
    Ok(messages
        .into_iter()
        .map(|item| Found {
            label: label.clone(),
            item,
        })
        .collect())
}

/// 从归档快照里还原后再检查.
///
/// 只还原 `db_storage/message/` 下的库与其 WAL, 用完即删, 不留明文副本。
fn from_archive(args: &RecallsArgs, root: &Path) -> anyhow::Result<Vec<Found>> {
    let archive =
        Archive::open(root).with_context(|| format!("打开归档目录失败: {}", root.display()))?;
    let snapshot = match args.snapshot {
        Some(created_at) => archive.load(created_at)?,
        None => archive
            .latest()?
            .context("归档里还没有任何快照; 先跑一次 wxmsg snap")?,
    };
    tracing::info!(
        created_at = snapshot.created_at,
        files = snapshot.files.len(),
        bytes = format_size(snapshot.total_bytes()),
        "已选定归档快照"
    );

    // 挑出消息库; `-wal` 与 `-shm` 单独处理。
    let databases: Vec<String> = snapshot
        .files
        .iter()
        .filter(|file| file.rel_path.starts_with("db_storage/message/"))
        .filter(|file| file.rel_path.ends_with(".db"))
        .map(|file| file.rel_path.clone())
        .collect();
    if databases.is_empty() {
        anyhow::bail!(
            "快照 {} 里没有 db_storage/message 下的库; 归档时是否用了别的 --prefix?",
            snapshot.created_at
        );
    }

    let restore_root = PathBuf::from(RESTORE_DIR).join(snapshot.created_at.to_string());
    if restore_root.exists() {
        std::fs::remove_dir_all(&restore_root).ok();
    }
    let keys = load_keys(&args.common)?;
    let mut sources = Vec::new();

    for rel_path in &databases {
        let restored = restore_root.join(rel_path);
        archive
            .restore(&snapshot, rel_path, &restored)
            .with_context(|| format!("还原 {rel_path} 失败"))?;

        // 归档里的 WAL 是找回旧页版本的关键。
        let wal_rel = format!("{rel_path}-wal");
        let wal_path = restore_root.join(&wal_rel);
        if snapshot.find(&wal_rel).is_some() {
            archive.restore(&snapshot, &wal_rel, &wal_path)?;
        } else {
            tracing::debug!(wal = %wal_rel, "快照里没有对应的 WAL");
        }

        let store = EncryptedStore::open_with_keys(&restored, &keys)
            .with_context(|| format!("打开还原出来的库失败: {}", restored.display()))?;
        let key = *keys
            .require(&restored.display().to_string())
            .with_context(|| format!("找不到 {rel_path} 对应的密钥"))?;
        sources.push(SourceDb {
            label: format!("归档 {} {rel_path}", snapshot.created_at),
            db_path: restored,
            wal_path,
            key,
            store,
        });
    }

    let mut found = Vec::new();
    for source in &sources {
        let label = source.label.clone();
        for item in source.recover(args.chat.as_deref())? {
            found.push(Found {
                label: label.clone(),
                item,
            });
        }
    }

    // 还原出来的明文副本含完整聊天记录, 用完就删。
    drop(sources);
    if let Err(error) = std::fs::remove_dir_all(&restore_root) {
        tracing::warn!(%error, path = %restore_root.display(), "清理还原目录失败, 请手动删除");
    } else {
        tracing::info!(path = %restore_root.display(), "已清理还原出来的明文副本");
    }
    Ok(found)
}

/// 描述一行是在哪里找到的.
fn describe_origin(origin: FoundIn) -> String {
    match origin {
        FoundIn::DbFile => "db_file".to_owned(),
        FoundIn::Wal { generation, frame } => format!("wal#{generation}/frame{frame}"),
    }
}

/// 一行的显示内容.
fn describe(found: &Found) -> Vec<String> {
    let item = &found.item;
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
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

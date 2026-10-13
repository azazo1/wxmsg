//! `plain` 子命令: 把加密库 (可含 WAL) 重建为明文副本, 并可自检是否与 SQLCipher 直连一致.
//!
//! 这个命令是排错与验证用的: 它绕过 SQLCipher 自己逐页解密, 因此能把解密链路与
//! SQLCipher 的结果直接对照。产物是含完整聊天记录的明文库, 用完应当删除。

use std::path::{Path, PathBuf};

use anyhow::Context;
use wxmsg_core::crypto::CipherParams;
use wxmsg_core::wal::Wal;
use wxmsg_core::{EncryptedStore, image, verify};

use crate::cli::PlainArgs;
use crate::commands::{load_keys, prepare_path};

pub fn run(args: PlainArgs) -> anyhow::Result<()> {
    let keys = load_keys(&args.common)?;
    let path = prepare_path(&args.common)?;
    let key = *keys
        .require(&path.display().to_string())
        .with_context(|| format!("找不到 {} 对应的密钥", path.display()))?;

    let wal_path = args
        .wal
        .clone()
        .unwrap_or_else(|| with_suffix(&path, "-wal"));
    let wal = Wal::open_if_present(&wal_path)
        .with_context(|| format!("解析 WAL 失败: {}", wal_path.display()))?;
    match wal.as_ref() {
        Some(wal) => tracing::info!(
            frames = wal.len(),
            generations = wal.generations.len(),
            current_frames = wal.current().map_or(0, |generation| generation.frames.len()),
            unparsed_bytes = wal.unparsed_bytes,
            path = %wal_path.display(),
            "已解析 WAL"
        ),
        None => tracing::info!(path = %wal_path.display(), "没有 WAL, 只解主库"),
    }

    tracing::warn!("明文副本包含完整聊天记录, 请在使用后自行删除");

    let report = image::build(
        &path,
        wal.as_ref(),
        key,
        CipherParams::wechat(),
        args.commit,
        &args.out,
    )
    .with_context(|| format!("重建明文副本失败: {}", path.display()))?;
    tracing::info!(
        base_pages = report.base_pages,
        frames_applied = report.frames_applied,
        pages = report.pages,
        commit = ?report.commit,
        out = %args.out.display(),
        "已重建明文副本"
    );

    if args.verify {
        if args.commit.is_some() {
            tracing::warn!("指定了 --commit, 自检结果会因状态不同而不一致");
        }
        verify(&path, &keys, &args.out)?;
    }
    Ok(())
}

/// 给路径加上后缀, 例如由 `message_1.db` 得到 `message_1.db-wal`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// 把明文副本与 SQLCipher 直连的结果逐表比对行数.
fn verify(encrypted: &Path, keys: &wxmsg_core::KeyStore, plain: &Path) -> anyhow::Result<()> {
    let store = EncryptedStore::open_with_keys(encrypted, keys)
        .with_context(|| format!("打开加密库失败: {}", encrypted.display()))?;

    let mismatches = verify::compare_with_plain(&store, plain)
        .with_context(|| format!("比对失败: {}", plain.display()))?;
    tracing::info!(
        tables = verify::table_count(&store)?,
        mismatches = mismatches.len(),
        "已逐表比对行数"
    );

    if !mismatches.is_empty() {
        for line in &mismatches {
            tracing::error!("{line}");
        }
        anyhow::bail!(
            "明文副本与 SQLCipher 直连的结果不一致, 共 {} 张表",
            mismatches.len()
        );
    }
    println!(
        "自检通过: {} 张表的行数与 SQLCipher 直连一致",
        verify::table_count(&store)?
    );
    Ok(())
}

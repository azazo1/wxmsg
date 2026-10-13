//! `plain` 子命令: 把加密库重建为明文副本, 并可自检是否与 SQLCipher 直连一致.
//!
//! 这个命令是排错与验证用的: 它绕过 SQLCipher 自己逐页解密, 因此能把解密链路与
//! SQLCipher 的结果直接对照。产物是含完整聊天记录的明文库, 用完应当删除。

use anyhow::Context;
use wxmsg_core::EncryptedStore;
use wxmsg_core::crypto::{CipherParams, rebuild_plaintext};
use wxmsg_core::verify;

use crate::cli::PlainArgs;
use crate::commands::{load_keys, prepare_path};

pub fn run(args: PlainArgs) -> anyhow::Result<()> {
    let keys = load_keys(&args.common)?;
    let path = prepare_path(&args.common)?;
    let key = *keys
        .require(&path.display().to_string())
        .with_context(|| format!("找不到 {} 对应的密钥", path.display()))?;

    tracing::warn!("明文副本包含完整聊天记录, 请在使用后自行删除");

    let report = rebuild_plaintext(&path, key, CipherParams::wechat(), &args.out)
        .with_context(|| format!("重建明文副本失败: {}", path.display()))?;
    tracing::info!(
        pages = report.pages,
        out = %args.out.display(),
        "已重建明文副本"
    );

    if args.verify {
        verify(&path, &keys, &args.out)?;
    }
    Ok(())
}

/// 把明文副本与 SQLCipher 直连的结果逐表比对行数.
fn verify(
    encrypted: &std::path::Path,
    keys: &wxmsg_core::KeyStore,
    plain: &std::path::Path,
) -> anyhow::Result<()> {
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

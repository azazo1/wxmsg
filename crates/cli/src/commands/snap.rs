//! `snap` 子命令: 取一次归档快照, 并按保留策略裁剪.
//!
//! 归档只存**密文**, 因此不需要密钥; 事后要还原内容时再拿密钥去解。

use serde_json::json;

use crate::cli::SnapArgs;
use crate::commands::archive::ArchiveJob;

pub fn run(args: SnapArgs) -> anyhow::Result<()> {
    let job = ArchiveJob::from_args(&args.archive)?;
    let bytes = job.take()?;
    let usage = job.archive.usage()?;
    let snapshots = job.archive.list()?.len();

    if args.archive.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "source": job.source.display().to_string(),
                "archive": job.archive.root().display().to_string(),
                "snapshot_bytes": bytes,
                "archive_bytes": usage,
                "snapshots": snapshots,
            }))?
        );
        return Ok(());
    }

    println!(
        "快照完成: {} 个文件, {}; 归档现有 {} 份快照, 合计 {}",
        job.archive
            .latest()?
            .map(|snapshot| snapshot.files.len())
            .unwrap_or(0),
        crate::commands::format_size(bytes),
        snapshots,
        crate::commands::format_size(usage),
    );
    Ok(())
}

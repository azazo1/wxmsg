//! 归档任务的公共装配.
//!
//! `snap` 与 `keep` 都用这里的 [`ArchiveJob`]: 解析路径, 组装过滤条件与保留策略, 然后取快照。

use std::path::{Path, PathBuf};

use anyhow::Context;
use wxmsg_core::archive::{Archive, Filter, PruneReport, Retention, now_seconds};

use crate::cli::ArchiveArgs;
use crate::commands::{format_size, parse_size};
use crate::paths;

/// 一次归档任务的全部配置.
pub struct ArchiveJob {
    /// 账号目录.
    pub source: PathBuf,
    /// 归档目录.
    pub archive: Archive,
    /// 挑哪些文件.
    pub filter: Filter,
    /// 保留策略.
    pub retention: Retention,
    /// 取完快照是否裁剪.
    pub prune: bool,
}

impl ArchiveJob {
    /// 由命令行参数组装.
    pub fn from_args(args: &ArchiveArgs) -> anyhow::Result<Self> {
        let source = match args.source.as_deref() {
            Some(path) => paths::normalize_source(path),
            None => paths::account_dir()?,
        };
        let root = match args.archive.as_deref() {
            Some(path) => path.to_path_buf(),
            None => paths::default_archive()?,
        };
        let archive = Archive::open(&root)
            .with_context(|| format!("打开归档目录失败: {}", root.display()))?;

        let mut filter = Filter::default();
        if !args.prefix.is_empty() {
            filter.prefixes = args.prefix.clone();
        }

        let retention = Retention {
            keep_days: args.keep_days,
            max_bytes: parse_size(&args.max_bytes).with_context(|| {
                format!("看不懂的容量写法: {}; 支持 K / M / G 后缀", args.max_bytes)
            })?,
        };

        Ok(Self {
            source,
            archive,
            filter,
            retention,
            prune: !args.no_prune,
        })
    }

    /// 这个路径是否在归档范围内.
    pub fn tracks(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.source) else {
            return false;
        };
        let relative = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        self.filter
            .prefixes
            .iter()
            .any(|prefix| relative.starts_with(prefix.as_str()))
    }

    /// 取一次快照 (并按需要裁剪), 返回清单记录的总字节数.
    pub fn take(&self) -> anyhow::Result<u64> {
        let started = std::time::Instant::now();
        let snapshot = self
            .archive
            .take(&self.source, &self.filter)
            .with_context(|| format!("对 {} 取快照失败", self.source.display()))?;
        tracing::info!(
            created_at = snapshot.created_at,
            files = snapshot.files.len(),
            bytes = format_size(snapshot.total_bytes()),
            elapsed_ms = started.elapsed().as_millis(),
            "已取快照"
        );

        if self.prune {
            let report = self.archive.prune(&self.retention, now_seconds())?;
            log_prune(&report, self);
        }
        Ok(snapshot.total_bytes())
    }
}

/// 打印裁剪结果.
pub fn log_prune(report: &PruneReport, job: &ArchiveJob) {
    if report.removed_snapshots == 0 && report.freed_bytes == 0 {
        return;
    }
    let usage = job.archive.usage().unwrap_or(0);
    tracing::info!(
        removed_snapshots = report.removed_snapshots,
        freed = format_size(report.freed_bytes),
        usage = format_size(usage),
        keep_days = job.retention.keep_days,
        max_bytes = format_size(job.retention.max_bytes),
        "已按保留策略裁剪"
    );
}

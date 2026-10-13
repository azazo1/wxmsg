//! 库镜像: 把加密库与 WAL 帧合成一份明文 SQLite 文件.
//!
//! SQLite 只呈现"当前状态", 看不到 WAL 里被取代的旧页面版本。把主库逐页解出之后, 再按帧序
//! 把 WAL 的页写回对应偏移, 就能得到 WAL 中任一时间点的完整库 —— 于是可以直接用 SQLite 去
//! 查那个时间点的数据, 不必自己解析 B 树。
//!
//! 这也是恢复撤回内容的关键: 把镜像停在撤回事务提交之前, 被删的那一行就还在。

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use crate::crypto::plaintext::rebuild_plaintext;
use crate::crypto::{CipherParams, KEY_SIZE, PageCipher};
use crate::error::{CoreError, Result};
use crate::wal::Wal;

/// 镜像生成结果.
#[derive(Debug, Clone, Copy)]
pub struct ImageReport {
    /// 主库本身的页数.
    pub base_pages: u64,
    /// 应用了多少个 WAL 帧.
    pub frames_applied: usize,
    /// 镜像最终的页数.
    pub pages: u64,
    /// 停在哪个提交帧 (0 起); 没有应用 WAL 时为 `None`.
    pub commit: Option<usize>,
}

/// 生成明文镜像.
///
/// `stop_after_commit` 指定停在 WAL 的第几个提交帧 (0 起), `None` 表示应用全部有效帧;
/// 没有提交帧或者没有 WAL 时只解出主库。
pub fn build(
    source: impl AsRef<Path>,
    wal: Option<&Wal>,
    key: [u8; KEY_SIZE],
    params: CipherParams,
    stop_after_commit: Option<usize>,
    target: impl AsRef<Path>,
) -> Result<ImageReport> {
    let source = source.as_ref();
    let target = target.as_ref();

    let base = rebuild_plaintext(source, key, params, target)?;
    let mut report = ImageReport {
        base_pages: base.pages as u64,
        frames_applied: 0,
        pages: base.pages as u64,
        commit: None,
    };

    let Some(wal) = wal else {
        return Ok(report);
    };
    // 只有当前代能安全地叠加到主库上: 更旧的那些代对应的主库版本早已不存在。
    let Some(generation) = wal.current() else {
        tracing::warn!("WAL 里没有当前代, 只解主库");
        return Ok(report);
    };
    let commits = generation.commits();
    if commits.is_empty() {
        tracing::warn!(
            frames = generation.frames.len(),
            "当前代没有提交帧, 只解主库"
        );
        return Ok(report);
    }

    // 默认停在最后一个提交帧, 即当前状态。
    let commit_index = stop_after_commit.unwrap_or(commits.len() - 1);
    let stop_frame = *commits
        .get(commit_index)
        .ok_or(crate::wal::WalError::CommitOutOfRange {
            index: commit_index,
            count: commits.len(),
        })?;

    let cipher = PageCipher::from_db_file(source, key, params)?;
    let page_size = params.page_size() as u64;
    let mut file = OpenOptions::new()
        .write(true)
        .open(target)
        .map_err(|error| CoreError::Io {
            path: target.to_path_buf(),
            source: error,
        })?;

    for frame in generation.frames.iter().take(stop_frame + 1) {
        let page = cipher.decrypt_page(frame.page_number as u64, &frame.page)?;
        file.seek(SeekFrom::Start((frame.page_number as u64 - 1) * page_size))
            .map_err(|error| CoreError::Io {
                path: target.to_path_buf(),
                source: error,
            })?;
        file.write_all(&page).map_err(|error| CoreError::Io {
            path: target.to_path_buf(),
            source: error,
        })?;
        report.frames_applied += 1;
    }

    // 提交帧给出的库页数就是那一刻的真实长度, 超出部分要截掉。
    let pages = generation.frames[stop_frame].db_size as u64;
    file.set_len(pages * page_size)
        .map_err(|error| CoreError::Io {
            path: target.to_path_buf(),
            source: error,
        })?;

    report.pages = pages;
    report.commit = Some(commit_index);
    Ok(report)
}

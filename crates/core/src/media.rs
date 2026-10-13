//! 媒体文件定位.
//!
//! 微信 4.x 的媒体目录有稳定的规律, 因此可以从消息本身算出文件在哪:
//!
//! ```text
//! msg/file/<年-月>/<原文件名>                                  文件消息
//! msg/attach/<会话 wxid 的 md5>/<年-月>/Img/<行号>_<时间>_t.dat  图片缩略图
//! msg/video/<年-月>/<md5>_thumb.jpg                            视频封面
//! ```
//!
//! 图片存的是微信自己的容器格式 (不是直接能看的 jpeg), 这里按原样导出, 不擅自"修复"。
//! 语音在 4.x 里没有稳定的本地命名规则, 因此暂不定位。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, TimeZone};

use crate::chat::message_table_name;

/// 账号目录下的媒体文件.
#[derive(Debug, Clone)]
pub struct MediaLocator {
    account: PathBuf,
}

impl MediaLocator {
    /// 以账号目录 (含 `msg/` 与 `db_storage/`) 为根.
    pub fn new(account: impl AsRef<Path>) -> Self {
        Self {
            account: account.as_ref().to_path_buf(),
        }
    }

    /// 账号目录.
    pub fn account(&self) -> &Path {
        &self.account
    }

    /// 年月目录名, 例如 `2026-10`.
    fn month(create_time: i64) -> String {
        match Local.timestamp_opt(create_time, 0).single() {
            Some(time) => time.format("%Y-%m").to_string(),
            // 时间戳离谱时就落到 1970-01, 至少不会拼出一个奇怪的路径。
            None => DateTime::from_timestamp(0, 0)
                .map(|time| time.format("%Y-%m").to_string())
                .unwrap_or_else(|| "1970-01".to_owned()),
        }
    }

    /// 文件消息的附件.
    pub fn file(&self, create_time: i64, name: &str) -> Option<PathBuf> {
        let path = self
            .account
            .join("msg/file")
            .join(Self::month(create_time))
            .join(name);
        path.is_file().then_some(path)
    }

    /// 图片的缩略图.
    ///
    /// 本地文件名有两种写法, 都与消息本身有关, 但用哪一个取决于图片来源:
    ///
    /// - `<行号>_<时间>_t.dat`
    /// - `<消息里的 md5>_t.dat`
    ///
    /// 两种都试不到时返回 `None`。此时导出只写 `[图片]`, **不会**拿同目录里的其它文件顶替,
    /// 免得把不相干的图配到消息上。
    pub fn image(
        &self,
        chat: &str,
        create_time: i64,
        local_id: i64,
        digest: Option<&str>,
    ) -> Option<PathBuf> {
        let table = message_table_name(chat);
        let table_digest = table.strip_prefix("Msg_").unwrap_or(&table);
        let directory = self
            .account
            .join("msg/attach")
            .join(table_digest)
            .join(Self::month(create_time))
            .join("Img");

        let mut candidates = vec![format!("{local_id}_{create_time}_t.dat")];
        if let Some(digest) = digest {
            candidates.push(format!("{digest}_t.dat"));
            candidates.push(format!("{digest}.dat"));
        }
        for name in candidates {
            let path = directory.join(name);
            if path.is_file() {
                return Some(path);
            }
        }

        // 第三种: 文件名里那个编号并不总是消息行号, 但**时间部分与消息一致**, 因此退一步按
        // `_<时间>_t.dat` 找。同一会话同一秒出现两张图的情形极少, 这个回退够用。
        let suffix = format!("_{create_time}_t.dat");
        let entries = std::fs::read_dir(&directory).ok()?;
        let mut matched: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(suffix.as_str()))
            })
            .collect();
        // 有多个候选时取文件名最小的那个, 保证每次导出结果一致。
        matched.sort();
        matched.into_iter().next()
    }

    /// 视频的封面 (按 md5 前缀在当月目录里找).
    pub fn video(&self, create_time: i64, digest: &str) -> Option<PathBuf> {
        let directory = self
            .account
            .join("msg/video")
            .join(Self::month(create_time));
        let entries = std::fs::read_dir(&directory).ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(digest) {
                return Some(entry.path());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_uses_local_time() {
        // 2026-08-27 09:27:37 本地时间。
        let month = MediaLocator::month(1787794057);
        assert_eq!(month.len(), 7);
        assert_eq!(&month[4..5], "-");
    }

    #[test]
    fn missing_files_are_none() {
        let locator = MediaLocator::new("/nonexistent-account-for-test");
        assert!(locator.file(0, "nothing.txt").is_none());
        assert!(locator.image("wxid_x", 0, 1, Some("deadbeef")).is_none());
        assert!(locator.video(0, "deadbeef").is_none());
    }
}

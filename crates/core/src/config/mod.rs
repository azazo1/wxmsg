//! 应用配置.
//!
//! 配置文件是 TOML, 默认位置 `~/Library/Application Support/wxmsg/config.toml`。
//! 所有字段都带默认值, 因此配置里只写想改的那几项即可。
//!
//! **每个字段都有默认值 + 带 `version` 字段** 是这里的关键约定: 前者保证旧配置缺字段也能读,
//! 后者让结构变化时能识别并迁移 (见 [`migrate`])。

pub mod migrate;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// 当前配置格式版本.
pub const CONFIG_VERSION: u32 = 1;

/// 应用配置.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// 配置格式版本.
    #[serde(default)]
    pub version: u32,
    /// 路径设置.
    #[serde(default)]
    pub paths: Paths,
    /// 归档设置.
    #[serde(default)]
    pub archive: ArchiveSettings,
}

/// 路径设置.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Paths {
    /// 账号目录; 留空表示自动探测.
    #[serde(default)]
    pub source: String,
    /// 归档目录; 留空表示用默认位置.
    #[serde(default)]
    pub archive: String,
    /// 密钥文件; 留空表示用 `secrets/all_keys.json`.
    #[serde(default)]
    pub keys: String,
}

/// 归档设置.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveSettings {
    /// 保留多少天.
    #[serde(default = "default_keep_days")]
    pub keep_days: i64,
    /// 总量上限, 支持 K / M / G 后缀.
    #[serde(default = "default_max_bytes")]
    pub max_bytes: String,
    /// 两次快照之间的最小间隔 (秒).
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    /// 最后一次变动之后静置多久才取快照 (秒).
    #[serde(default = "default_debounce_secs")]
    pub debounce_secs: u64,
    /// 只归档这些相对路径.
    #[serde(default = "default_prefixes")]
    pub prefixes: Vec<String>,
}

fn default_keep_days() -> i64 {
    7
}

fn default_max_bytes() -> String {
    "8G".to_owned()
}

fn default_interval_secs() -> u64 {
    300
}

fn default_debounce_secs() -> u64 {
    3
}

fn default_prefixes() -> Vec<String> {
    vec!["db_storage".to_owned()]
}

impl Default for ArchiveSettings {
    fn default() -> Self {
        Self {
            keep_days: default_keep_days(),
            max_bytes: default_max_bytes(),
            interval_secs: default_interval_secs(),
            debounce_secs: default_debounce_secs(),
            prefixes: default_prefixes(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            paths: Paths::default(),
            archive: ArchiveSettings::default(),
        }
    }
}

impl Config {
    /// 默认配置文件位置: `~/Library/Application Support/wxmsg/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library/Application Support/wxmsg")
                .join("config.toml"),
        )
    }

    /// 读取配置; 文件不存在时返回默认配置.
    ///
    /// 忽略 `--config` 与默认路径的区别: 调用方传进来的路径不存在时, 这里会如实返回默认值
    /// 并留下一条日志, 而不是报错, 因为"还没写过配置"是完全正常的状态。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            tracing::debug!(path = %path.display(), "没有配置文件, 用默认值");
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|source| CoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let config = migrate::migrate(&text).map_err(|reason| CoreError::Config {
            path: path.to_path_buf(),
            reason,
        })?;
        tracing::debug!(path = %path.display(), version = config.version, "已读取配置");
        Ok(config)
    }

    /// 写出配置.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| CoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = toml::to_string_pretty(self).map_err(|error| CoreError::Config {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
        std::fs::write(path, text).map_err(|source| CoreError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// 账号目录: 配置里写了就用它, 否则返回 `None` 表示交给调用方自动探测.
    pub fn source_dir(&self) -> Option<PathBuf> {
        non_empty(&self.paths.source).map(PathBuf::from)
    }

    /// 归档目录.
    pub fn archive_dir(&self) -> Option<PathBuf> {
        non_empty(&self.paths.archive).map(PathBuf::from)
    }

    /// 密钥文件.
    pub fn keys_file(&self) -> Option<PathBuf> {
        non_empty(&self.paths.keys).map(PathBuf::from)
    }
}

/// 空字符串按"没设置"处理.
fn non_empty(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_uses_defaults() {
        let config = migrate::migrate("version = 1\n[archive]\nkeep_days = 3\n").expect("应能解析");
        assert_eq!(config.archive.keep_days, 3);
        // 没写的字段取默认值。
        assert_eq!(config.archive.max_bytes, "8G");
        assert_eq!(config.archive.interval_secs, 300);
        assert_eq!(config.archive.prefixes, vec!["db_storage".to_owned()]);
    }

    #[test]
    fn empty_config_is_valid() {
        let config = migrate::migrate("").expect("空配置也应能解析");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config, Config::default());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut config = Config::default();
        config.paths.source = "/tmp/example".to_owned();
        config.archive.keep_days = 2;
        let text = toml::to_string_pretty(&config).expect("应能序列化");
        assert_eq!(migrate::migrate(&text).expect("应能解析"), config);
    }
}

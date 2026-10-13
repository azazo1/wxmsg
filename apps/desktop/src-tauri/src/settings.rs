//! 界面设置.
//!
//! 存在数据目录里的一个小 JSON。字段都带默认值, 因此旧文件缺字段也能读。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 设置项.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// 启动时只留在托盘, 不显示主窗口.
    #[serde(default)]
    pub start_hidden: bool,
    /// 主窗口隐藏时, 是否同时隐藏 dock 图标.
    #[serde(default = "default_true")]
    pub hide_dock_when_hidden: bool,
    /// 上次查看的会话, 启动时直接选中.
    #[serde(default)]
    pub last_chat: String,
    /// 消息列表一次显示多少条.
    #[serde(default = "default_limit")]
    pub message_limit: usize,
}

fn default_true() -> bool {
    true
}

fn default_limit() -> usize {
    300
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            start_hidden: false,
            hide_dock_when_hidden: true,
            last_chat: String::new(),
            message_limit: default_limit(),
        }
    }
}

impl Settings {
    /// 从数据目录读取.
    pub fn load(data_dir: &Path) -> Self {
        let path = Self::path(data_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => settings,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "设置文件读不动, 用默认值");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// 写回数据目录.
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        let path = Self::path(data_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text)
    }

    /// 设置文件位置.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.json")
    }
}

//! 默认位置探测.
//!
//! 微信的数据根固定在容器里, 账号目录名形如 `<wxid>_<后缀>`。这里把这些约定收在一处,
//! 命令行不传路径时就用探测结果, 免得每次都要手打一长串路径。

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

/// 微信容器的数据根.
pub fn wechat_root() -> Option<PathBuf> {
    let home = home_dir()?;
    Some(home.join("Library/Containers/com.tencent.xinWeChat/Data/Documents/xwechat_files"))
}

/// 用户主目录.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// 默认归档目录: `~/Library/Application Support/wxmsg/archive`.
///
/// 归档含完整聊天记录, 属于用户数据, 因此放在标准的应用支持目录里, 而不是临时目录。
pub fn default_archive() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("读不到 HOME 环境变量")?;
    Ok(home.join("Library/Application Support/wxmsg/archive"))
}

/// 默认密钥文件: 当前目录下的 `secrets/all_keys.json`.
pub fn default_keys() -> PathBuf {
    PathBuf::from("secrets/all_keys.json")
}

/// 探测账号目录: 数据根下含 `db_storage` 且最近被写过的那个.
pub fn account_dir() -> anyhow::Result<PathBuf> {
    let root = wechat_root().context("读不到 HOME 环境变量")?;
    if !root.is_dir() {
        bail!(
            "找不到微信数据根 {}; 请用 --source 指定账号目录",
            root.display()
        );
    }

    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries =
        std::fs::read_dir(&root).with_context(|| format!("读取 {} 失败", root.display()))?;
    for entry in entries {
        let entry = entry?;
        let candidate = entry.path();
        let db_storage = candidate.join("db_storage");
        if !db_storage.is_dir() {
            continue;
        }
        let modified = std::fs::metadata(&db_storage)
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(current, _)| modified > *current) {
            best = Some((modified, candidate));
        }
    }

    match best {
        Some((_, path)) => Ok(path),
        None => bail!(
            "{} 下没有找到含 db_storage 的账号目录; 请用 --source 指定",
            root.display()
        ),
    }
}

/// 把 `--source` 归一化: 既接受账号目录, 也接受数据根或 `db_storage` 本身.
///
/// 归档时以"账号目录"为根, 过滤条件里的相对路径 (默认 `db_storage`) 才成立。
pub fn normalize_source(source: &Path) -> PathBuf {
    if source.join("db_storage").is_dir() {
        return source.to_path_buf();
    }
    if source.file_name().is_some_and(|name| name == "db_storage")
        && let Some(parent) = source.parent()
    {
        return parent.to_path_buf();
    }
    source.to_path_buf()
}

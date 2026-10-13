//! 密钥库.
//!
//! 微信为每个数据库使用**独立的 32 字节原始密钥**, 密钥只在登录态的进程内、于加解密瞬间
//! 可被截获 (详见 `docs/handover.md` 的 "密钥从哪里来")。本模块只负责保存与查询,
//! 不做抓取, 也不做任何网络操作。
//!
//! 文件格式与抓取工具的输出保持一致, 便于两边直接对接:
//!
//! ```json
//! {
//!   "message/message_1.db": { "enc_key": "3d20c2dc...(64 位十六进制)" },
//!   "contact/contact.db": { "enc_key": "..." }
//! }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crypto::{CipherParams, KEY_SIZE, verify_key};
use crate::error::{CoreError, Result};
use crate::hex;

/// 密钥历史格式版本.
pub const KEY_HISTORY_VERSION: u32 = 1;

/// 单个数据库的密钥记录.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEntry {
    /// 十六进制形式的 32 字节原始密钥.
    pub enc_key: String,
}

/// 一次密钥抓取.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyCapture {
    /// 抓取时间 (Unix 秒).
    pub captured_at: i64,
    /// 这次抓到的库与密钥.
    pub entries: BTreeMap<String, KeyEntry>,
}

/// 密钥历史.
///
/// 微信更新, 换号或迁移记录都会让密钥轮换, 因此密钥要**带时间累积**保存, 而不是覆盖。
/// 归档里的密文因此永远不会因为"密钥换了"而变成废纸。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyHistory {
    /// 格式版本.
    pub version: u32,
    /// 历次抓取, 按时间升序.
    pub captures: Vec<KeyCapture>,
}

impl Default for KeyHistory {
    fn default() -> Self {
        Self {
            version: KEY_HISTORY_VERSION,
            captures: Vec::new(),
        }
    }
}

impl KeyHistory {
    /// 从 JSON 文件加载; 文件不存在时返回空历史.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|source| CoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let history: Self = serde_json::from_str(&text).map_err(|source| CoreError::KeyFile {
            path: path.to_path_buf(),
            source,
        })?;
        if history.version != KEY_HISTORY_VERSION {
            return Err(CoreError::Archive(format!(
                "密钥历史版本 {} 不受支持",
                history.version
            )));
        }
        Ok(history)
    }

    /// 保存为 JSON 文件.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let text = serde_json::to_string_pretty(self).map_err(|source| CoreError::KeyFile {
            path: PathBuf::from(path),
            source,
        })?;
        std::fs::write(path, text).map_err(|source| CoreError::Io {
            path: PathBuf::from(path),
            source,
        })
    }

    /// 追加一次抓取; 与已有条目完全相同时不记录, 免得历史里堆一堆重复项.
    pub fn append(&mut self, captured_at: i64, keys: &KeyStore) -> bool {
        let entries: BTreeMap<String, KeyEntry> = keys
            .entries
            .iter()
            .map(|(name, key)| {
                (
                    name.clone(),
                    KeyEntry {
                        enc_key: hex::encode(key),
                    },
                )
            })
            .collect();
        if entries.is_empty() {
            return false;
        }
        if self
            .captures
            .last()
            .is_some_and(|last| last.entries == entries)
        {
            return false;
        }
        self.captures.push(KeyCapture {
            captured_at,
            entries,
        });
        true
    }

    /// 历次抓取的数量.
    pub fn len(&self) -> usize {
        self.captures.len()
    }

    /// 是否没有任何记录.
    pub fn is_empty(&self) -> bool {
        self.captures.is_empty()
    }

    /// 用 HMAC 找出能打开这个库的密钥.
    ///
    /// 从最近的抓取往前找, 命中即返回。库改名, 复制或换目录都不影响匹配。
    pub fn key_for(
        &self,
        db_path: impl AsRef<Path>,
        params: CipherParams,
    ) -> Result<Option<[u8; KEY_SIZE]>> {
        let db_path = db_path.as_ref();
        for capture in self.captures.iter().rev() {
            for entry in capture.entries.values() {
                let key = crate::crypto::key_from_hex(&entry.enc_key)?;
                if verify_key(db_path, key, params)? {
                    return Ok(Some(key));
                }
            }
        }
        Ok(None)
    }

    /// 扫描一个目录下的库, 用历史里的密钥逐个匹配, 得到一份可直接用的密钥集合.
    ///
    /// 键名是相对 `root` 的路径, 与 [`KeyStore`] 的约定一致。
    pub fn match_directory(
        &self,
        root: impl AsRef<Path>,
        params: CipherParams,
    ) -> Result<KeyStore> {
        let root = root.as_ref();
        let mut store = KeyStore::default();
        for path in collect_databases(root)? {
            let Some(key) = self.key_for(&path, params)? else {
                tracing::debug!(path = %path.display(), "没有匹配的密钥");
                continue;
            };
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            store.insert_key(&rel, key);
        }
        Ok(store)
    }
}

/// 递归找目录下的 `.db` 文件.
fn collect_databases(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|source| CoreError::Io {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| CoreError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|extension| extension == "db") {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// 以库的相对路径为键的密钥集合, 例如 `message/message_1.db`.
#[derive(Debug, Clone, Default)]
pub struct KeyStore {
    entries: BTreeMap<String, [u8; 32]>,
}

impl KeyStore {
    /// 从 JSON 文件加载密钥.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| CoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let raw: BTreeMap<String, KeyEntry> =
            serde_json::from_str(&text).map_err(|source| CoreError::KeyFile {
                path: path.to_path_buf(),
                source,
            })?;

        let mut store = Self::default();
        for (name, entry) in raw {
            store.insert_hex(&name, &entry.enc_key)?;
        }
        Ok(store)
    }

    /// 插入一条密钥, 密钥为十六进制字符串.
    pub fn insert_hex(&mut self, name: &str, hex_text: &str) -> Result<()> {
        let bytes = hex::decode(hex_text).ok_or_else(|| CoreError::KeyLength {
            name: name.to_owned(),
        })?;
        if bytes.len() != 32 {
            return Err(CoreError::KeyLength {
                name: name.to_owned(),
            });
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        self.entries.insert(name.to_owned(), key);
        Ok(())
    }

    /// 插入一条已经有字节形式的密钥.
    pub fn insert_key(&mut self, name: &str, key: [u8; KEY_SIZE]) {
        self.entries.insert(name.to_owned(), key);
    }

    /// 查询密钥.
    ///
    /// 依次尝试三种匹配, 以便调用方用任意一种写法指代同一个库:
    ///
    /// 1. 完整键名精确匹配, 例如 `message/message_1.db`
    /// 2. 路径尾部匹配, 例如绝对路径 `/tmp/wx/message/message_1.db`
    /// 3. 文件名匹配, 例如库被复制成 `.tmp/db/message_1.db` 之后路径层级已经不同;
    ///    仅当同名库**只有一个**时才接受, 避免张冠李戴
    pub fn get(&self, name: &str) -> Option<&[u8; 32]> {
        if let Some(key) = self.entries.get(name) {
            return Some(key);
        }

        let target = Path::new(name);
        if let Some((_, key)) = self
            .entries
            .iter()
            .find(|(candidate, _)| target.ends_with(Path::new(candidate.as_str())))
        {
            return Some(key);
        }

        let file_name = target.file_name()?;
        let mut hits = self
            .entries
            .iter()
            .filter(|(candidate, _)| Path::new(candidate.as_str()).file_name() == Some(file_name));
        let (_, key) = hits.next()?;
        if hits.next().is_some() {
            tracing::warn!(
                name,
                "密钥文件中有多个同名库, 无法按键名判定, 请用完整相对路径指定"
            );
            return None;
        }
        Some(key)
    }

    /// 按键名取密钥, 找不到时报错.
    pub fn require(&self, name: &str) -> Result<&[u8; 32]> {
        self.get(name)
            .ok_or_else(|| CoreError::KeyMissing(name.to_owned()))
    }

    /// 已登记的密钥条目数.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否没有任何密钥.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 遍历所有库名.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// 已登记的库名列表.
    pub fn name_list(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// 保存为 JSON 文件.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let mut out = BTreeMap::new();
        for (name, key) in &self.entries {
            out.insert(
                name.clone(),
                KeyEntry {
                    enc_key: hex::encode(key),
                },
            );
        }
        let text = serde_json::to_string_pretty(&out).map_err(|source| CoreError::KeyFile {
            path: PathBuf::from(path),
            source,
        })?;
        std::fs::write(path, text).map_err(|source| CoreError::Io {
            path: PathBuf::from(path),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_matches_by_suffix() {
        let mut store = KeyStore::default();
        store
            .insert_hex("message/message_1.db", &"ab".repeat(32))
            .expect("密钥应可解析");
        assert_eq!(store.len(), 1);
        assert!(store.get("message/message_1.db").is_some());
        assert!(store.get("message_1.db").is_some());
        assert!(store.get("/tmp/x/message/message_1.db").is_some());
        // 库被复制到别处后路径层级不同, 靠文件名仍然能找到。
        assert!(store.get(".tmp/db/message_1.db").is_some());
        assert!(store.get("message_0.db").is_none());
    }

    #[test]
    fn rejects_ambiguous_file_name() {
        let mut store = KeyStore::default();
        store
            .insert_hex("a/message_1.db", &"ab".repeat(32))
            .expect("密钥应可解析");
        store
            .insert_hex("b/message_1.db", &"cd".repeat(32))
            .expect("密钥应可解析");
        // 两个同名库: 不能用文件名判定。
        assert!(store.get("elsewhere/message_1.db").is_none());
        // 但完整路径仍然可用。
        assert!(store.get("a/message_1.db").is_some());
    }

    #[test]
    fn rejects_wrong_length() {
        let mut store = KeyStore::default();
        assert!(store.insert_hex("a.db", "abcd").is_err());
    }
}

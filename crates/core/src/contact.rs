//! 联系人.
//!
//! 导出时把内部 wxid 换成人看得懂的名字: 备注 > 昵称 > 微信号 > wxid。
//! 群聊成员也用同一张表查, 因此群消息的发送者同样能显示成人名。

use std::collections::HashMap;
use std::path::Path;

use crate::error::{CoreError, Result};
use crate::keys::KeyStore;
use crate::store::EncryptedStore;

/// 一个联系人.
#[derive(Debug, Clone, Default)]
pub struct ContactEntry {
    /// 内部 wxid.
    pub username: String,
    /// 微信号.
    pub alias: Option<String>,
    /// 备注名.
    pub remark: Option<String>,
    /// 昵称.
    pub nick_name: Option<String>,
}

impl ContactEntry {
    /// 显示名: 备注 > 昵称 > 微信号 > wxid.
    pub fn display_name(&self) -> String {
        for candidate in [&self.remark, &self.nick_name, &self.alias] {
            if let Some(name) = candidate
                && !name.trim().is_empty()
            {
                return name.clone();
            }
        }
        self.username.clone()
    }
}

/// 联系人表.
#[derive(Debug, Clone, Default)]
pub struct ContactBook {
    entries: HashMap<String, ContactEntry>,
}

impl ContactBook {
    /// 从 `contact.db` 读取.
    pub fn load(path: impl AsRef<Path>, keys: &KeyStore) -> Result<Self> {
        let path = path.as_ref();
        let store = EncryptedStore::open_with_keys(path, keys)?;
        Self::from_store(&store)
    }

    /// 从一个已经打开的库里读取联系人表.
    pub fn from_store(store: &EncryptedStore) -> Result<Self> {
        if !store.has_table("contact")? {
            tracing::debug!("库里没有 contact 表, 只用 wxid 显示");
            return Ok(Self::default());
        }

        // 列名在不同版本里略有差异, 缺哪列就当它是空的。
        let mut statement = store.connection().prepare("PRAGMA table_info(contact)")?;
        let mut columns = Vec::new();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            columns.push(name);
        }

        let pick = |wanted: &str| -> String {
            if columns.iter().any(|name| name == wanted) {
                wanted.to_owned()
            } else {
                "NULL".to_owned()
            }
        };
        let sql = format!(
            "SELECT username, {}, {}, {} FROM contact",
            pick("alias"),
            pick("remark"),
            pick("nick_name")
        );

        let mut statement = store.connection().prepare(&sql)?;
        let mut rows = statement.query([])?;
        let mut entries = HashMap::new();
        while let Some(row) = rows.next()? {
            let username: Option<String> = row.get(0)?;
            let Some(username) = username else {
                continue;
            };
            entries.insert(
                username.clone(),
                ContactEntry {
                    username,
                    alias: row.get(1)?,
                    remark: row.get(2)?,
                    nick_name: row.get(3)?,
                },
            );
        }
        tracing::info!(count = entries.len(), "已读取联系人");
        Ok(Self { entries })
    }

    /// 查一个 wxid 的显示名; 查不到就原样返回 wxid.
    pub fn display_name(&self, username: &str) -> String {
        match self.entries.get(username) {
            Some(entry) => entry.display_name(),
            None => username.to_owned(),
        }
    }

    /// 联系人数量.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 在账号目录里找一个库文件: 先按相对路径找, 找不到就在 `db_storage` 下按文件名找.
pub fn find_database(account: impl AsRef<Path>, relative: &str) -> Result<std::path::PathBuf> {
    let account = account.as_ref();
    let direct = account.join(relative);
    if direct.is_file() {
        return Ok(direct);
    }

    let name = Path::new(relative)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative.to_owned());
    let root = account.join("db_storage");
    let mut stack = vec![root.clone()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|found| found == name.as_str()) {
                return Ok(path);
            }
        }
    }
    Err(CoreError::Archive(format!(
        "在 {} 下找不到 {}",
        account.display(),
        relative
    )))
}

//! 加密库的打开与基础查询.
//!
//! 微信的库是 SQLCipher 4: AES-256-CBC, 页 4096 字节, 每页末 80 字节为 IV 与 HMAC,
//! 每个库使用独立的 32 字节原始密钥。密钥以 `x'<hex>'` 形式交给 SQLCipher,
//! **不手工解页**, 也不需要重新实现 HMAC 校验 (由 SQLCipher 完成)。
//!
//! 打开一律使用只读标志。SQLite 在只读打开 WAL 模式的库时可能需要 `-shm` 可写,
//! 因此读取微信正在使用的真实库之前, 应先用 [`copy_database`] 把库连同 WAL 复制到工作目录。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::error::{CoreError, Result};
use crate::hex;
use crate::keys::KeyStore;

/// 已打开的加密库.
#[derive(Debug)]
pub struct EncryptedStore {
    conn: Connection,
    path: PathBuf,
}

impl EncryptedStore {
    /// 用指定的 32 字节原始密钥打开库.
    pub fn open(path: impl AsRef<Path>, key: &[u8; 32]) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&path, flags).map_err(|source| CoreError::Open {
            path: path.clone(),
            source,
        })?;

        // 密钥必须先设置, 之后才能执行任何语句。
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex::encode(key)))
            .map_err(|source| CoreError::Open {
                path: path.clone(),
                source,
            })?;

        // 读一次模式表: 密钥不对时 SQLCipher 会在这里报 "file is not a database"。
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|source| match source {
            rusqlite::Error::SqliteFailure(_, ref message)
                if message
                    .as_deref()
                    .is_some_and(|text| text.contains("not a database")) =>
            {
                CoreError::WrongKey {
                    name: path.display().to_string(),
                }
            }
            other => CoreError::Open {
                path: path.clone(),
                source: other,
            },
        })?;

        Ok(Self { conn, path })
    }

    /// 用密钥库中的密钥打开库, 键名可以是相对路径或文件名。
    pub fn open_with_keys(path: impl AsRef<Path>, keys: &KeyStore) -> Result<Self> {
        let path = path.as_ref();
        let name = path.display().to_string();
        let key = keys.require(&name)?;
        Self::open(path, key)
    }

    /// 底层连接.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// 库的路径.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 列出全部表名.
    pub fn tables(&self) -> Result<Vec<String>> {
        let mut statement = self
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut names = Vec::new();
        for row in rows {
            names.push(row?);
        }
        Ok(names)
    }

    /// 表是否存在.
    pub fn has_table(&self, name: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

/// 把库及其 WAL 复制到目标目录, 返回新路径.
///
/// 只读微信正在使用的库有风险: SQLite 可能需要写 `-shm`, 微信自身也在写 WAL。
/// 先复制再打开可以避免任何写入落到微信的数据目录。
pub fn copy_database(source: impl AsRef<Path>, target_dir: impl AsRef<Path>) -> Result<PathBuf> {
    let source = source.as_ref();
    let target_dir = target_dir.as_ref();
    std::fs::create_dir_all(target_dir).map_err(|error| CoreError::Io {
        path: target_dir.to_path_buf(),
        source: error,
    })?;

    let file_name = source
        .file_name()
        .ok_or_else(|| CoreError::KeyMissing(source.display().to_string()))?;
    let target = target_dir.join(file_name);

    copy_if_exists(source, &target)?;
    for suffix in ["-wal", "-shm"] {
        let from = with_suffix(source, suffix);
        let to = with_suffix(&target, suffix);
        copy_if_exists(&from, &to)?;
    }
    Ok(target)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn copy_if_exists(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|error| CoreError::Io {
        path: to.to_path_buf(),
        source: error,
    })?;
    Ok(())
}

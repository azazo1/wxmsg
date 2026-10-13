//! 明文副本与 SQLCipher 直连结果的一致性检查.
//!
//! 两边走的是完全不同的代码路径: 一边是本库自己的页解密, 一边是 SQLCipher 连接。
//! 逐表行数全部一致, 才能说明对页格式与加密参数的理解是对的。

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::chat::quote_identifier;
use crate::error::{CoreError, Result};
use crate::store::EncryptedStore;

/// 统计连接里每张表的行数.
fn counts(connection: &Connection) -> Result<BTreeMap<String, i64>> {
    let mut names = Vec::new();
    {
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            names.push(row?);
        }
    }

    let mut counts = BTreeMap::new();
    for name in names {
        let count: i64 = connection.query_row(
            &format!("SELECT count(*) FROM {}", quote_identifier(&name)),
            [],
            |row| row.get(0),
        )?;
        counts.insert(name, count);
    }
    Ok(counts)
}

/// 把明文副本的每张表行数与 SQLCipher 直连的结果比对.
///
/// 返回差异描述, 空表示完全一致。
pub fn compare_with_plain(store: &EncryptedStore, plain: impl AsRef<Path>) -> Result<Vec<String>> {
    let plain = plain.as_ref();
    let expected = table_counts(store)?;

    let connection =
        Connection::open_with_flags(plain, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|source| {
            CoreError::Open {
                path: plain.to_path_buf(),
                source,
            }
        })?;
    let actual = counts(&connection)?;

    let mut mismatches = Vec::new();
    for (name, count) in &expected {
        match actual.get(name) {
            Some(other) if other == count => {}
            Some(other) => {
                mismatches.push(format!("{name}: SQLCipher {count} 行, 明文副本 {other} 行"))
            }
            None => mismatches.push(format!("{name}: 明文副本里没有这张表")),
        }
    }
    for name in actual.keys() {
        if !expected.contains_key(name) {
            mismatches.push(format!("{name}: 只存在于明文副本"));
        }
    }
    Ok(mismatches)
}

/// 加密库连接里每张表的行数.
pub fn table_counts(store: &EncryptedStore) -> Result<BTreeMap<String, i64>> {
    counts(store.connection())
}

/// 参与比对的表数量, 用于日志.
pub fn table_count(store: &EncryptedStore) -> Result<usize> {
    Ok(table_counts(store)?.len())
}

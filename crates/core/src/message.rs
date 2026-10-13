//! 消息读取与正文解码.
//!
//! 正文列 `message_content` 的格式是 `"<发送者 wxid>:\n<载荷>"`。当同行
//! `WCDB_CT_message_content` 为 4 时, 载荷是 zstd 压缩的二进制 (帧头 `28 b5 2f fd`);
//! 图片, 文件, 引用等富内容本身又是 XML 文本。
//!
//! `local_type` 的高位打包了子类型, 真实类型取低 32 位, **不能直接与 1 / 3 / 10000 比较**。

use std::io::Read;

use rusqlite::types::ValueRef;

use crate::chat::{message_table_name, quote_identifier};
use crate::error::{CoreError, Result};
use crate::store::EncryptedStore;

/// zstd 帧头.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// `WCDB_CT_message_content` 为 4 时表示 zstd 压缩.
const CONTENT_FLAG_ZSTD: i64 = 4;

/// 一条原始消息.
#[derive(Debug, Clone)]
pub struct RawMessage {
    /// 会话内的本地编号.
    pub local_id: i64,
    /// 打包了子类型的类型字段.
    pub local_type: i64,
    /// 创建时间 (Unix 秒).
    pub create_time: i64,
    /// 原始正文字节.
    pub content: Option<Vec<u8>>,
    /// 正文的压缩标记.
    pub content_flag: Option<i64>,
    /// `real_sender_id`: 本分片 `Name2Id` 的行号, 指向真正的发送者.
    ///
    /// 正文里的 `发送者:` 前缀**不可靠**: 一对一聊天里根本没有这个前缀。要判断一条消息是谁
    /// 发的, 应当看这个字段。
    pub sender_id: Option<i64>,
}

impl RawMessage {
    /// 真实类型: 取低 32 位.
    pub fn real_type(&self) -> i64 {
        self.local_type & 0xffff_ffff
    }

    /// 是否为撤回通知 (系统消息 10000).
    pub fn is_recall_notice(&self) -> bool {
        self.real_type() == 10_000
    }

    /// 解压后的完整正文, 仍带有 `"<发送者>:\n"` 前缀.
    pub fn payload(&self) -> Result<String> {
        let Some(raw) = self.content.as_ref() else {
            return Ok(String::new());
        };
        let needs_decompress =
            self.content_flag == Some(CONTENT_FLAG_ZSTD) || raw.starts_with(&ZSTD_MAGIC);
        let bytes = if needs_decompress {
            let mut out = Vec::new();
            zstd::stream::read::Decoder::new(raw.as_slice())
                .map_err(CoreError::Decompress)?
                .read_to_end(&mut out)
                .map_err(CoreError::Decompress)?;
            out
        } else {
            raw.clone()
        };
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// 发送者 wxid.
    pub fn sender(&self) -> Result<Option<String>> {
        let payload = self.payload()?;
        Ok(split_sender(&payload).0.map(str::to_owned))
    }

    /// 去掉发送者前缀后的正文.
    pub fn text(&self) -> Result<String> {
        let payload = self.payload()?;
        Ok(split_sender(&payload).1.to_owned())
    }
}

/// 拆分 `"<发送者 wxid>:\n<正文>"`.
///
/// 只把第一行且以冒号结尾、且不含空格的内容当作发送者, 避免把正文里的冒号误判成前缀。
pub fn split_sender(payload: &str) -> (Option<&str>, &str) {
    let Some(index) = payload.find('\n') else {
        return (None, payload);
    };
    let head = &payload[..index];
    if head.ends_with(':') && !head.contains(' ') && head.len() <= 64 {
        (Some(&head[..head.len() - 1]), &payload[index + 1..])
    } else {
        (None, payload)
    }
}

/// 读取某个会话最近的消息, 按时间倒序.
pub fn recent_messages(
    store: &EncryptedStore,
    username: &str,
    limit: usize,
) -> Result<Vec<RawMessage>> {
    let table = message_table_name(username);
    if !store.has_table(&table)? {
        return Ok(Vec::new());
    }

    let columns = message_columns(store, &table)?;
    let sql = format!(
        "SELECT {columns} FROM {} ORDER BY create_time DESC, local_id DESC LIMIT ?1",
        quote_identifier(&table)
    );

    let mut statement = store.connection().prepare(&sql)?;
    let rows = statement.query_map([limit as i64], |row| {
        Ok(RawMessage {
            local_id: row.get(0)?,
            local_type: row.get(1)?,
            create_time: row.get(2)?,
            content: read_content(row, 3)?,
            content_flag: row.get(4)?,
            sender_id: row.get(5)?,
        })
    })?;

    let mut messages = Vec::new();
    for row in rows {
        messages.push(row?);
    }
    Ok(messages)
}

/// 按时间顺序遍历整个会话的消息.
///
/// 全量导出一个会话可能有几十万条, 因此不一次性装进内存, 而是逐条交给调用方处理。
/// 返回处理过的条数。
pub fn for_each_message(
    store: &EncryptedStore,
    username: &str,
    mut handler: impl FnMut(&RawMessage) -> Result<()>,
) -> Result<usize> {
    let table = message_table_name(username);
    if !store.has_table(&table)? {
        return Ok(0);
    }

    let columns = message_columns(store, &table)?;
    let sql = format!(
        "SELECT {columns} FROM {} ORDER BY create_time ASC, local_id ASC",
        quote_identifier(&table)
    );

    let mut statement = store.connection().prepare(&sql)?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let message = RawMessage {
            local_id: row.get(0)?,
            local_type: row.get(1)?,
            create_time: row.get(2)?,
            content: read_content(row, 3)?,
            content_flag: row.get(4)?,
            sender_id: row.get(5)?,
        };
        handler(&message)?;
        count += 1;
    }
    Ok(count)
}

/// 读一条消息需要的列.
///
/// 固定取前 6 列: 行号, 类型, 时间, 正文, 压缩标记, 发送者编号。缺列时用 `NULL` 顶上,
/// 免得为不同版本的表结构写两套读取代码。
fn message_columns(store: &EncryptedStore, table: &str) -> Result<String> {
    let flag = if has_column(store, table, "WCDB_CT_message_content")? {
        "WCDB_CT_message_content"
    } else {
        "NULL"
    };
    let sender = if has_column(store, table, "real_sender_id")? {
        "real_sender_id"
    } else {
        "NULL"
    };
    Ok(format!(
        "local_id, local_type, create_time, message_content, {flag}, {sender}"
    ))
}

/// 按 SQLite 的动态类型读取一列, 统一得到原始字节.
///
/// `message_content` 的存储类型**不固定**: 未压缩的消息是 TEXT, 而
/// `WCDB_CT_message_content = 4` 时是 zstd 压缩后的 BLOB。直接按 `Vec<u8>` 取会在 TEXT 上
/// 报类型错误, 所以这里显式分派。是否解压交给调用方判断。
fn read_content(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    Ok(match row.get_ref(index)? {
        ValueRef::Null => None,
        ValueRef::Integer(value) => Some(value.to_string().into_bytes()),
        ValueRef::Real(value) => Some(value.to_string().into_bytes()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(bytes.to_vec()),
    })
}

/// 表是否包含某列.
pub fn has_column(store: &EncryptedStore, table: &str, column: &str) -> Result<bool> {
    let sql = format!("PRAGMA table_info({})", quote_identifier(table));
    let mut statement = store.connection().prepare(&sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sender_prefix() {
        assert_eq!(split_sender("wxid_abc:\n你好"), (Some("wxid_abc"), "你好"));
        // 正文里的冒号不应被当前缀。
        assert_eq!(
            split_sender("时间: 2026-10-13\n正文"),
            (None, "时间: 2026-10-13\n正文")
        );
    }

    #[test]
    fn real_type_ignores_packed_high_bits() {
        let message = RawMessage {
            local_id: 1,
            local_type: (7_i64 << 32) | 10_000,
            create_time: 0,
            content: None,
            content_flag: None,
            sender_id: None,
        };
        assert_eq!(message.real_type(), 10_000);
        assert!(message.is_recall_notice());
    }
}

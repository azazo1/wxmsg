//! 会话与消息表命名.
//!
//! 微信把每个会话的消息放在**独立表**里, 表名为 `Msg_<md5(username)>`, 其中 `username`
//! 是内部 wxid, 不是微信号别名。同一个账号的消息还会按时间分片到 `message_0.db` 与
//! `message_1.db`, 两个分片各自维护一份会话编号映射表 `Name2Id`, 编号空间互不相通。

use md5::{Digest, Md5};

use crate::error::Result;
use crate::hex;
use crate::store::EncryptedStore;

/// 计算某个 wxid 对应的消息表名.
pub fn message_table_name(username: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(username.as_bytes());
    // md-5 0.11 的 `finalize` 返回固定长度数组, 不再实现 `LowerHex`, 因此显式取切片编码。
    format!("Msg_{}", hex::encode(hasher.finalize().as_slice()))
}

/// 一个会话条目.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatEntry {
    /// 会话在本分片中的编号, 即 `Name2Id` 的 rowid.
    pub session_id: i64,
    /// 内部 wxid.
    pub username: String,
    /// 本分片中该会话的消息数; 表不存在时为 0.
    pub message_count: i64,
    /// 本分片中是否存在该会话的消息表.
    pub has_table: bool,
    /// 最近一条消息的时间 (Unix 秒); 表不存在或表为空时为 `None`.
    ///
    /// 按此字段倒序排列, 顺序即与微信会话列表一致。
    pub last_message_time: Option<i64>,
}

impl ChatEntry {
    /// 该会话的消息表名.
    pub fn table_name(&self) -> String {
        message_table_name(&self.username)
    }
}

/// 列出本分片中的全部会话, 并统计各自的消息条数.
///
/// 找不到 `Name2Id` 时返回空列表, 便于调用方对非消息库统一处理。
pub fn list_chats(store: &EncryptedStore) -> Result<Vec<ChatEntry>> {
    if !store.has_table("Name2Id")? {
        return Ok(Vec::new());
    }

    let mut statement = store
        .connection()
        .prepare("SELECT rowid, user_name FROM Name2Id ORDER BY rowid")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut chats = Vec::new();
    for row in rows {
        let (session_id, username) = row?;
        let table = message_table_name(&username);
        let mut entry = ChatEntry {
            session_id,
            username,
            message_count: 0,
            has_table: false,
            last_message_time: None,
        };

        if store.has_table(&table)? {
            entry.has_table = true;
            // 统计与取最近时间合并成一次扫描, 避免对同一张表扫两遍。
            let (count, last) = store.connection().query_row(
                &format!(
                    "SELECT count(*), max(create_time) FROM {}",
                    quote_identifier(&table)
                ),
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
            )?;
            entry.message_count = count;
            entry.last_message_time = last;
        }
        chats.push(entry);
    }
    Ok(chats)
}

/// 把标识符用双引号包起来, 避免表名中的特殊字符影响语句解析.
pub(crate) fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_name_matches_wechat_rule() {
        // 校验办法: 微信把附件放在 md5(username) 命名的目录里, 同一个哈希函数。
        assert_eq!(
            message_table_name("filehelper"),
            "Msg_9e20f478899dc29eb19741386f9343c8"
        );
    }
}

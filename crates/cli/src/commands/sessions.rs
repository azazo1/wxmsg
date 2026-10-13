//! `sessions` 子命令: 列出库中的会话.
//!
//! 输出按最近一条消息的时间倒序, 顺序与微信自己的会话列表一致。
//! 注意每个消息分片 (`message_0.db` / `message_1.db`) 各自维护一份会话编号, 因此要看到
//! 完整列表需要对两个分片分别执行。

use serde_json::json;
use wxmsg_core::list_chats;

use crate::cli::SessionsArgs;
use crate::commands::{format_time, open_store, table};

pub fn run(args: SessionsArgs) -> anyhow::Result<()> {
    let store = open_store(&args.common)?;
    let mut chats = list_chats(&store)?;

    let total_messages: i64 = chats.iter().map(|chat| chat.message_count).sum();
    let with_table = chats.iter().filter(|chat| chat.has_table).count();
    tracing::info!(
        sessions = chats.len(),
        with_table,
        messages = total_messages,
        "已统计本分片的会话"
    );

    if args.min_messages > 0 {
        chats.retain(|chat| chat.message_count >= args.min_messages);
    }
    // 没有消息的会话排在最后: `None` 比任何时间都小, 倒序时自然沉底。
    chats.sort_by_key(|chat| std::cmp::Reverse(chat.last_message_time));
    chats.truncate(args.limit);

    if chats.is_empty() {
        tracing::warn!("没有符合条件的会话");
    }

    if args.common.json {
        let payload: Vec<_> = chats
            .iter()
            .map(|chat| {
                json!({
                    "session_id": chat.session_id,
                    "username": chat.username,
                    "table": chat.table_name(),
                    "has_table": chat.has_table,
                    "message_count": chat.message_count,
                    "last_message_time": chat.last_message_time,
                    "last_message_at": chat
                        .last_message_time
                        .map(|seconds| format_time(Some(seconds))),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let rows: Vec<Vec<String>> = chats
        .iter()
        .map(|chat| {
            vec![
                chat.session_id.to_string(),
                chat.message_count.to_string(),
                format_time(chat.last_message_time),
                chat.username.clone(),
            ]
        })
        .collect();
    println!(
        "{}",
        table::render(
            &["会话号", "消息数", "最近消息", "用户名"],
            &[true, true, false, false],
            &rows,
        )
    );
    Ok(())
}

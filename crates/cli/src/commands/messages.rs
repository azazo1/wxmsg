//! `messages` 子命令: 读取某个会话的最近消息.
//!
//! 会话用**内部 wxid** 指定 (不是微信号别名, 也不是备注名)。别名到 wxid 的映射在
//! `contact.db` 的 `contact` 表里, 解析功能留到导出阶段再做。

use serde_json::json;
use wxmsg_core::{message_table_name, recent_messages};

use crate::cli::MessagesArgs;
use crate::commands::{format_time, open_store, table};

/// 表格里正文列的最大显示宽度.
const TEXT_WIDTH: usize = 100;

pub fn run(args: MessagesArgs) -> anyhow::Result<()> {
    let store = open_store(&args.common)?;

    let table_name = message_table_name(&args.chat);
    if !store.has_table(&table_name)? {
        anyhow::bail!(
            "库中没有 {0} 的消息表 {1}; 请确认 --chat 用的是内部 wxid (形如 wxid_xxx), 不是微信号别名或备注名",
            args.chat,
            table_name
        );
    }

    let messages = recent_messages(&store, &args.chat, args.limit)?;
    let notices = messages
        .iter()
        .filter(|message| message.is_recall_notice())
        .count();
    tracing::info!(
        chat = %args.chat,
        table = %table_name,
        count = messages.len(),
        recalls = notices,
        "已读取消息"
    );

    if args.common.json {
        let mut payload = Vec::with_capacity(messages.len());
        for message in &messages {
            payload.push(json!({
                "local_id": message.local_id,
                "local_type": message.local_type,
                "real_type": message.real_type(),
                "create_time": message.create_time,
                "create_time_at": format_time(Some(message.create_time)),
                "sender": message.sender()?,
                "text": message.text()?,
                "is_recall_notice": message.is_recall_notice(),
            }));
        }
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let mut rows = Vec::with_capacity(messages.len());
    for message in &messages {
        let mut text = one_line(&message.text()?);
        if message.is_recall_notice() {
            text = format!("[撤回通知] {text}");
        }
        rows.push(vec![
            message.local_id.to_string(),
            format_time(Some(message.create_time)),
            message.real_type().to_string(),
            message.sender()?.unwrap_or_else(|| "-".to_owned()),
            table::truncate(&text, TEXT_WIDTH),
        ]);
    }

    if rows.is_empty() {
        println!("该会话在本分片中没有消息");
        return Ok(());
    }

    println!(
        "{}",
        table::render(
            &["行号", "时间", "类型", "发送者", "正文"],
            &[true, false, true, false, false],
            &rows,
        )
    );
    Ok(())
}

/// 把正文压成单行: 表格里换行会破坏对齐, JSON 输出则保留原文.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

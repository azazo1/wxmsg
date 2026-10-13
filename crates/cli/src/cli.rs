//! 命令行参数定义.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// 微信本地记录工具.
#[derive(Debug, Parser)]
#[command(name = "wxmsg", version, about, long_about = None)]
pub struct Cli {
    /// 子命令.
    #[command(subcommand)]
    pub command: Command,
}

/// 子命令.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 列出消息库中的会话及其消息条数.
    Sessions(SessionsArgs),

    /// 读取某个会话的最近消息.
    Messages(MessagesArgs),
}

/// 各子命令共用的参数.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// 数据库路径.
    #[arg(long, value_name = "PATH")]
    pub db: PathBuf,

    /// 密钥文件 (JSON), 形如 {"message/message_1.db": {"enc_key": "..."}}.
    #[arg(long, value_name = "PATH")]
    pub keys: PathBuf,

    /// 打开前先把库连同 WAL 复制到该目录.
    ///
    /// 微信正在使用的库不建议直接打开: SQLite 可能需要写 `-shm`.
    #[arg(long, value_name = "DIR")]
    pub copy_to: Option<PathBuf>,

    /// 以 JSON 输出.
    #[arg(long)]
    pub json: bool,
}

/// `sessions` 子命令.
#[derive(Debug, Args)]
pub struct SessionsArgs {
    /// 共用参数.
    #[command(flatten)]
    pub common: CommonArgs,

    /// 只显示消息数不少于该值的会话.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub min_messages: i64,

    /// 最多显示多少个会话.
    #[arg(long, default_value_t = 50, value_name = "N")]
    pub limit: usize,
}

/// `messages` 子命令.
#[derive(Debug, Args)]
pub struct MessagesArgs {
    /// 共用参数.
    #[command(flatten)]
    pub common: CommonArgs,

    /// 会话的 wxid.
    #[arg(long, value_name = "WXID")]
    pub chat: String,

    /// 最多显示多少条.
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub limit: usize,
}

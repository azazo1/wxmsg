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

    /// 把加密库重建为明文副本, 用于排错与自检.
    Plain(PlainArgs),

    /// 从 WAL 里找回被撤回 (被删除) 的消息.
    Recalls(RecallsArgs),
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

/// `plain` 子命令.
#[derive(Debug, Args)]
pub struct PlainArgs {
    /// 共用参数.
    #[command(flatten)]
    pub common: CommonArgs,

    /// 明文副本的输出路径.
    #[arg(long, value_name = "PATH")]
    pub out: PathBuf,

    /// WAL 文件路径; 默认取与库同名的 `-wal`, 不存在则只解主库.
    #[arg(long, value_name = "PATH")]
    pub wal: Option<PathBuf>,

    /// 只应用到 WAL 的第 N 个提交帧 (从 0 开始, 0 是最早的提交).
    ///
    /// 不指定时应用全部有效帧, 即当前状态。
    #[arg(long, value_name = "N")]
    pub commit: Option<usize>,

    /// 重建后与 SQLCipher 直连的结果逐表比对行数.
    ///
    /// 两边走的是不同的代码路径, 行数全部一致才能说明页格式与参数理解正确。
    /// 只在应用全部帧时有意义 (SQLCipher 看到的是当前状态)。
    #[arg(long)]
    pub verify: bool,
}

/// `recalls` 子命令.
#[derive(Debug, Args)]
pub struct RecallsArgs {
    /// 共用参数.
    #[command(flatten)]
    pub common: CommonArgs,

    /// 只看某个会话, 值为内部 wxid.
    #[arg(long, value_name = "WXID")]
    pub chat: Option<String>,

    /// WAL 文件路径; 默认取与库同名的 `-wal`.
    ///
    /// 被删消息的旧页版本就在这个文件里, 换一个 WAL 就等于换一个时间窗口。
    #[arg(long, value_name = "PATH")]
    pub wal: Option<std::path::PathBuf>,

    /// 最多显示多少条 (按时间取最近的若干条); 0 表示全部.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub limit: usize,
}

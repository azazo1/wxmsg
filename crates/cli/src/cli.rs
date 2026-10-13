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

    /// 对账号目录取一次归档快照 (只存密文).
    Snap(SnapArgs),

    /// 常驻监听, 按间隔取归档快照.
    Keep(KeepArgs),
}

/// 各子命令共用的参数.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// 数据库路径.
    ///
    /// 只有 `recalls --archive` 这一种用法不需要它。
    #[arg(long, value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// 密钥文件 (JSON), 形如 {"message/message_1.db": {"enc_key": "..."}}.
    ///
    /// 默认 `secrets/all_keys.json`。
    #[arg(long, value_name = "PATH")]
    pub keys: Option<PathBuf>,

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

    /// 从归档快照里找回, 值为归档目录.
    ///
    /// 不指定时直接看实时库。归档里的密文是持续留存的, 因此能回溯到更早的时间。
    #[arg(long, value_name = "DIR", conflicts_with = "wal")]
    pub archive: Option<std::path::PathBuf>,

    /// 指定归档快照的创建时间 (清单文件名里的整数); 不指定时用最新那份.
    #[arg(long, value_name = "TS", requires = "archive")]
    pub snapshot: Option<i64>,

    /// 最多显示多少条 (按时间取最近的若干条); 0 表示全部.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub limit: usize,
}

/// 归档相关参数, `snap` 与 `keep` 共用.
#[derive(Debug, Args)]
pub struct ArchiveArgs {
    /// 账号目录; 不指定时自动探测.
    #[arg(long, value_name = "DIR")]
    pub source: Option<PathBuf>,

    /// 归档目录; 默认 `~/Library/Application Support/wxmsg/archive`.
    #[arg(long, value_name = "DIR")]
    pub archive: Option<PathBuf>,

    /// 只归档这些相对路径 (默认 `db_storage`), 可重复.
    #[arg(long = "prefix", value_name = "REL")]
    pub prefix: Vec<String>,

    /// 归档保留多少天.
    #[arg(long, default_value_t = 7, value_name = "DAYS")]
    pub keep_days: i64,

    /// 归档总量上限, 支持 K / M / G 后缀.
    #[arg(long, default_value = "8G", value_name = "SIZE")]
    pub max_bytes: String,

    /// 只取快照, 不做保留策略裁剪.
    #[arg(long)]
    pub no_prune: bool,

    /// 以 JSON 输出.
    #[arg(long)]
    pub json: bool,
}

/// `snap` 子命令.
#[derive(Debug, Args)]
pub struct SnapArgs {
    /// 归档参数.
    #[command(flatten)]
    pub archive: ArchiveArgs,
}

/// `keep` 子命令.
#[derive(Debug, Args)]
pub struct KeepArgs {
    /// 归档参数.
    #[command(flatten)]
    pub archive: ArchiveArgs,

    /// 两次快照之间的最小间隔 (秒).
    #[arg(long, default_value_t = 300, value_name = "SECS")]
    pub interval: u64,

    /// 最后一次文件变动之后静置多久才取快照 (秒).
    #[arg(long, default_value_t = 3, value_name = "SECS")]
    pub debounce: u64,

    /// 只取一次快照就退出, 便于交给 launchd 之类定时器调用.
    #[arg(long)]
    pub once: bool,
}

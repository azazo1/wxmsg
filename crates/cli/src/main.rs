//! wxmsg 命令行入口.
//!
//! 只做三件事: 初始化日志, 解析参数, 分发到子命令. 具体逻辑放在 [`commands`] 各模块内。

mod cli;
mod commands;
mod paths;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

fn main() -> ExitCode {
    init_tracing();

    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // 用 `{error:#}` 打印 anyhow 的完整上下文链, 便于定位是哪一层失败。
            tracing::error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

/// 初始化日志.
///
/// 默认级别 `info`, 细节诊断可用 `RUST_LOG=wxmsg=debug` 打开。
/// 日志写 stderr, stdout 留给结果输出, 这样 `--json` 可以安全地重定向到文件。
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Sessions(args) => commands::sessions::run(args),
        Command::Messages(args) => commands::messages::run(args),
        Command::Plain(args) => commands::plain::run(args),
        Command::Recalls(args) => commands::recalls::run(args),
        Command::Snap(args) => commands::snap::run(args),
        Command::Keep(args) => commands::keep::run(args),
    }
}

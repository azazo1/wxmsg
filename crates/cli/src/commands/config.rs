//! `config` 子命令: 查看或初始化配置文件.
//!
//! 配置里每一项都有默认值, 因此大多数时候不需要写配置文件; 这个命令主要是把"当前到底生效了
//! 什么"摊开给人看。

use serde_json::json;
use wxmsg_core::config::Config;

use crate::cli::ConfigArgs;
use crate::commands::config_path;
use crate::paths;

pub fn run(args: ConfigArgs) -> anyhow::Result<()> {
    let path = config_path(args.config.as_deref())?;
    let config = Config::load(&path)?;

    if args.init {
        if path.exists() {
            println!("配置文件已存在, 没有覆盖: {}", path.display());
            return Ok(());
        }
        config.save(&path)?;
        println!("已写入配置文件: {}", path.display());
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": path.display().to_string(),
                "exists": path.exists(),
                "version": config.version,
                "paths": {
                    "source": config.paths.source,
                    "archive": config.paths.archive,
                    "keys": config.paths.keys,
                },
                "archive": {
                    "keep_days": config.archive.keep_days,
                    "max_bytes": config.archive.max_bytes,
                    "interval_secs": config.archive.interval_secs,
                    "debounce_secs": config.archive.debounce_secs,
                    "prefixes": config.archive.prefixes,
                },
                // 把最终会用到的那几个路径也一并给出来, 免得读者自己拼。
                "resolved": {
                    "source": config
                        .source_dir()
                        .map(|dir| dir.display().to_string()),
                    "archive": config
                        .archive_dir()
                        .map(|dir| dir.display().to_string())
                        .or_else(|| paths::default_archive().ok().map(|dir| dir.display().to_string())),
                    "keys": config
                        .keys_file()
                        .unwrap_or_else(paths::default_keys)
                        .display()
                        .to_string(),
                },
            }))?
        );
        return Ok(());
    }

    println!(
        "配置文件: {} {}",
        path.display(),
        if path.exists() {
            "(已存在)"
        } else {
            "(还没有, 用内置默认)"
        }
    );
    println!("\n--- 当前生效的配置 ---");
    println!("{}", toml::to_string_pretty(&config)?);

    // 空字符串表示"自动决定", 这里把最终会用到的路径摊开, 免得读者自己拼。
    println!("--- 解析后的实际路径 ---");
    match config.source_dir() {
        Some(dir) => println!("账号目录: {}", dir.display()),
        None => println!("账号目录: 自动探测"),
    }
    match config.archive_dir() {
        Some(dir) => println!("归档目录: {}", dir.display()),
        None => match paths::default_archive() {
            Ok(dir) => println!("归档目录: {} (默认位置)", dir.display()),
            Err(error) => println!("归档目录: 无法确定 ({error})"),
        },
    }
    match config.keys_file() {
        Some(file) => println!("密钥文件: {}", file.display()),
        None => println!("密钥文件: {} (默认位置)", paths::default_keys().display()),
    }
    Ok(())
}

//! `keep` 子命令: 常驻监听, 按间隔取归档快照.
//!
//! 之所以要常驻: 撤回删掉的行只存在于旧页版本里, 而 WAL 是固定大小的环形文件, 旧帧迟早会被
//! 冲掉。按固定间隔把密文留存下来, 就相当于给"当时那一刻"留了底片。
//!
//! 触发条件是"文件有变化"且"距上次快照超过 `--interval`", 并在最后一次变化后静置
//! `--debounce` 秒才动手, 避免一次检查点写出几十个快照。

use std::time::{Duration, Instant};

use anyhow::Context;
use notify::{RecursiveMode, Watcher, recommended_watcher};

use crate::cli::KeepArgs;
use crate::commands::archive::ArchiveJob;
use crate::commands::format_size;

pub fn run(args: KeepArgs) -> anyhow::Result<()> {
    let job = ArchiveJob::from_args(&args.archive)?;

    if args.once {
        let bytes = job.take()?;
        tracing::info!(bytes = format_size(bytes), "只做一次, 结束");
        return Ok(());
    }

    let (sender, receiver) = std::sync::mpsc::channel();
    let mut watcher = recommended_watcher(move |result| {
        let _ = sender.send(result);
    })
    .context("创建文件监听失败")?;
    watcher
        .watch(&job.source, RecursiveMode::Recursive)
        .with_context(|| format!("监听 {} 失败", job.source.display()))?;

    let config = crate::commands::load_config(args.archive.config.as_deref())?;
    let interval =
        Duration::from_secs(args.interval.unwrap_or(config.archive.interval_secs).max(1));
    let debounce = Duration::from_secs(args.debounce.unwrap_or(config.archive.debounce_secs));
    tracing::info!(
        source = %job.source.display(),
        archive = %job.archive.root().display(),
        interval_secs = interval.as_secs(),
        debounce_secs = debounce.as_secs(),
        "开始监听, Ctrl-C 结束"
    );

    // 启动先记一份基线, 并把节流窗口推到过去, 于是第一次判断就会立刻动手。
    let mut dirty_at = Some(Instant::now());
    let mut last = Instant::now() - interval;
    loop {
        while let Ok(event) = receiver.try_recv() {
            match event {
                Ok(event) => {
                    if event.paths.iter().any(|path| job.tracks(path)) {
                        dirty_at = Some(Instant::now());
                    }
                }
                Err(error) => tracing::warn!(%error, "文件监听报错"),
            }
        }

        let now = Instant::now();
        let quiet = dirty_at.is_some_and(|at| now.duration_since(at) >= debounce);
        let throttled = now.duration_since(last) < interval;
        if quiet && !throttled {
            match job.take() {
                Ok(_) => {
                    last = now;
                    dirty_at = None;
                }
                Err(error) => {
                    // 单次失败不该让监听退出: 微信可能正好在写, 下一轮再试。
                    tracing::error!("{error:#}");
                    last = now;
                    dirty_at = Some(now);
                }
            }
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}

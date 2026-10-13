//! `capture` 子命令: 一次性抓取库密钥.
//!
//! 为什么必须"复制一份微信再启动": 官方的 `WeChat.app` 按 家目录 + `Library/Containers/<bundle id>`
//! 拼出数据根, 所以只要把副本的 `HOME` 指到工作目录, 整个登录态与数据库都会落在工作目录里,
//! **真实的 `~/Library` 不会有任何写入**, 临时目录删掉即无痕。实机验证过这一点。
//!
//! 流程分五步, 每步都有日志: 校验环境 -> 复制副本 -> 启动副本并挂钩子 -> 用 HMAC 把候选密钥
//! 配到各个库 -> 合并写回密钥文件与密钥历史。
//!
//! 唯一需要人动手的是**扫一次码**: 密钥只在登录态、于加解密瞬间出现。

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::Deserialize;
use wxmsg_core::archive::now_seconds;
use wxmsg_core::crypto::{CipherParams, key_from_hex, verify_key};
use wxmsg_core::keys::{KeyHistory, KeyStore};

use crate::cli::CaptureArgs;
use crate::commands::load_config;
use crate::paths;

/// 微信默认安装位置.
const DEFAULT_APP: &str = "/Applications/WeChat.app";
/// 副本里可执行文件的位置.
const EXECUTABLE: &str = "Contents/MacOS/WeChat";

pub fn run(args: CaptureArgs) -> anyhow::Result<()> {
    // 一、校验环境。
    let app = args
        .app
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_APP));
    let binary = app.join(EXECUTABLE);
    if !binary.is_file() {
        bail!(
            "找不到微信可执行文件 {}; 用 --app 指定 WeChat.app 的位置",
            binary.display()
        );
    }
    let sidecar = PathBuf::from("sidecar/capture.py");
    if !sidecar.is_file() {
        bail!("找不到 {}; 请在仓库根目录运行本命令", sidecar.display());
    }

    let work = args
        .work
        .clone()
        .unwrap_or_else(|| PathBuf::from(".tmp/capture"));
    let home = work.join("home");
    std::fs::create_dir_all(&home)
        .with_context(|| format!("创建工作目录失败: {}", home.display()))?;

    // 二、复制副本。家目录保留下来, 于是下次抓取不必重新扫码。
    let copy = work.join("WeChat.app");
    if !copy.join(EXECUTABLE).is_file() {
        tracing::info!(
            source = %app.display(),
            target = %copy.display(),
            "正在复制微信副本, 体积较大, 请稍等"
        );
        copy_tree(&app, &copy)?;
        tracing::info!("副本已就绪");
    } else {
        tracing::info!(target = %copy.display(), "副本已存在, 直接使用");
    }

    // 三、跑 sidecar 收集候选密钥。
    let keys_json = work.join("keys.json");
    run_sidecar(&args, &sidecar, &copy, &home, &keys_json)?;

    // 四、读回候选密钥, 用 HMAC 判断哪把开哪个库。
    let candidates = read_candidates(&keys_json)?;
    if candidates.is_empty() {
        bail!("这次没有抓到任何候选密钥; 请确认副本站起来之后完成了登录 (密钥只在登录态出现)");
    }
    let db_root = find_db_storage(&home)?;
    let matched = match_keys(&db_root, &candidates)?;
    if matched.is_empty() {
        bail!(
            "抓到 {} 个候选密钥, 但没有一个能打开 {} 下的库; 可能登录还没完成",
            candidates.len(),
            db_root.display()
        );
    }
    for (rel_path, key) in &matched {
        // 密钥本身不进日志, 只留个能对照的前缀。
        tracing::info!(db = %rel_path, key_head = %redact(key), "已匹配到密钥");
    }

    // 五、合并写回。
    let config = load_config(args.common.config.as_deref())?;
    let keys_path = args
        .common
        .keys
        .clone()
        .or_else(|| config.keys_file())
        .unwrap_or_else(paths::default_keys);
    let mut store = KeyStore::load(&keys_path)?;
    let before = store.len();
    for (rel_path, key) in &matched {
        store.insert_key(rel_path, *key);
    }
    store.save(&keys_path)?;
    tracing::info!(
        path = %keys_path.display(),
        before,
        after = store.len(),
        "密钥文件已更新"
    );

    let history_path = history_path(&keys_path);
    let mut history = KeyHistory::load(&history_path)?;
    if history.append(now_seconds(), &store) {
        history.save(&history_path)?;
        tracing::info!(
            path = %history_path.display(),
            captures = history.len(),
            "密钥历史已追加一条"
        );
    } else {
        tracing::info!("与上次抓取完全相同, 密钥历史不需要追加");
    }

    println!(
        "完成: 这次匹配到 {} 个库的密钥 (候选 {} 个); 密钥文件 {}",
        matched.len(),
        candidates.len(),
        keys_path.display()
    );
    println!(
        "副本与副本家目录都留在 {}, 下次抓取可以直接复用, 不必重新扫码",
        work.display()
    );
    Ok(())
}

/// 递归复制目录.
fn copy_tree(source: &Path, target: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(target)
        .with_context(|| format!("创建目录失败: {}", target.display()))?;
    let mut files = 0usize;
    for entry in
        std::fs::read_dir(source).with_context(|| format!("读取目录失败: {}", source.display()))?
    {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).with_context(|| format!("复制失败: {}", from.display()))?;
            files += 1;
            if files.is_multiple_of(2000) {
                tracing::info!(files, "复制中");
            }
        }
    }
    Ok(())
}

/// 调用 sidecar 收集密钥.
fn run_sidecar(
    args: &CaptureArgs,
    sidecar: &Path,
    copy: &Path,
    home: &Path,
    out: &Path,
) -> anyhow::Result<()> {
    // 传给 frida 的路径必须是绝对的: 相对路径会被它当成应用标识去找, 报一个很费解的错误。
    let binary =
        std::fs::canonicalize(copy.join(EXECUTABLE)).unwrap_or_else(|_| copy.join(EXECUTABLE));
    // frida 由 sidecar 自己的 pyproject 声明, uv 会把它装进 sidecar/.venv。
    let mut command = std::process::Command::new("uv");
    command
        .arg("run")
        .arg("--project")
        .arg("sidecar")
        .arg("python")
        .arg(sidecar)
        .arg("--hook")
        .arg("sidecar/hook.js")
        .arg("--out")
        .arg(out)
        .arg("--timeout")
        .arg(args.timeout.to_string());

    if let Some(pid) = args.pid {
        command.arg("--pid").arg(pid.to_string());
    } else {
        command.arg("--target").arg(&binary).arg("--home").arg(home);
    }
    if args.quiet_sidecar {
        command.arg("--quiet");
    }

    tracing::info!(
        target = %binary.display(),
        home = %home.display(),
        "正在启动微信副本并注入钩子; 接下来请在副本窗口里扫码登录"
    );
    let status = command
        .status()
        .context("启动 sidecar 失败; 请确认 uv 已在 PATH 里")?;
    if !status.success() {
        bail!("sidecar 退出码 {status}; 它的输出在上面");
    }
    Ok(())
}

/// sidecar 写出来的候选密钥.
#[derive(Debug, Deserialize)]
struct CandidateFile {
    /// 候选密钥.
    #[serde(default)]
    keys: Vec<Candidate>,
}

/// 一个候选密钥.
#[derive(Debug, Deserialize)]
struct Candidate {
    /// 十六进制密钥.
    hex: String,
    /// 从哪些钩子点看到.
    #[serde(default)]
    tags: Vec<String>,
}

/// 读回候选密钥, 顺手去重.
fn read_candidates(path: &Path) -> anyhow::Result<Vec<[u8; 32]>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("读取 {} 失败", path.display()))?;
    let file: CandidateFile =
        serde_json::from_str(&text).with_context(|| format!("解析 {} 失败", path.display()))?;

    let mut keys = Vec::new();
    let mut tags = std::collections::BTreeSet::new();
    for candidate in &file.keys {
        for tag in &candidate.tags {
            tags.insert(tag.clone());
        }
    }
    for candidate in file.keys {
        match key_from_hex(&candidate.hex) {
            Ok(key) => {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            Err(error) => {
                tracing::warn!(hex_len = candidate.hex.len(), %error, "跳过长度不对的候选")
            }
        }
    }
    tracing::info!(
        candidates = keys.len(),
        hooks = %tags.into_iter().collect::<Vec<_>>().join(", "),
        "候选密钥的来源钩子点"
    );
    Ok(keys)
}

/// 找到副本家目录里的 `db_storage`.
fn find_db_storage(home: &Path) -> anyhow::Result<PathBuf> {
    let containers = home.join("Library/Containers");
    let mut found = Vec::new();
    let mut stack = vec![containers.clone()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.file_name().is_some_and(|name| name == "db_storage") {
                found.push(path);
                continue;
            }
            stack.push(path);
        }
    }
    found.sort();
    match found.first() {
        Some(path) => Ok(path.clone()),
        None => bail!(
            "副本家目录 {} 里还没有 db_storage; 说明还没登录成功",
            home.display()
        ),
    }
}

/// 用 HMAC 校验把候选密钥配到各个库.
fn match_keys(db_root: &Path, candidates: &[[u8; 32]]) -> anyhow::Result<Vec<(String, [u8; 32])>> {
    let databases = collect_databases(db_root)?;
    tracing::info!(
        databases = databases.len(),
        candidates = candidates.len(),
        "开始逐个库试密钥"
    );

    let params = CipherParams::wechat();
    let mut matched = Vec::new();
    for path in databases {
        let rel_path = path
            .strip_prefix(db_root)
            .unwrap_or(&path)
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        for key in candidates {
            if verify_key(&path, *key, params).unwrap_or(false) {
                matched.push((rel_path, *key));
                break;
            }
        }
    }
    Ok(matched)
}

/// 找一个目录下全部的 `.db`.
fn collect_databases(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .with_context(|| format!("读取目录失败: {}", directory.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "db") {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// 密钥历史的位置: 与密钥文件同目录, 叫 `keys-history.json`.
fn history_path(keys_path: &Path) -> PathBuf {
    keys_path
        .parent()
        .map(|parent| parent.join("keys-history.json"))
        .unwrap_or_else(|| PathBuf::from("keys-history.json"))
}

/// 密钥的短标记: 只留前 8 位十六进制, 便于人工对照, 不足以还原密钥.
fn redact(key: &[u8; 32]) -> String {
    key.iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        + "..."
}

# wxmsg 常用任务.
#
# 本项目只处理本机当前账号的本地数据, 边界与风险见 docs/handover.md.

# 密钥文件位置, 可用环境变量 WXMSG_KEYS 覆盖.
keys := env("WXMSG_KEYS", "secrets/all_keys.json")

[private]
default:
    @just --list

# 构建工作区.
build:
    cargo build

# 运行单元测试.
test:
    cargo test

# 静态检查, 警告视为错误.
lint:
    cargo clippy --all-targets -- -D warnings

# 格式化代码.
# 格式化改动应当单独提交, 不要和功能改动混在一个 commit 里.
fmt:
    cargo fmt

# 扫描仓库是否混入个人信息或密钥.
# 专属敏感词写在 secrets/private-terms.txt, 该文件不入库.
check-private:
    python3 scripts/check-private.py

# 扫描全部 git 历史是否含敏感内容.
# 推送前与推送后各跑一次; 若命中说明历史里进过敏感内容, 需要重写历史.
history-scan:
    python3 scripts/check-private.py --history

# 列出某个消息分片中的会话.
# 示例: just sessions .tmp/db/message_1.db --limit 20
sessions db *args:
    cargo run -q -p wxmsg-cli -- sessions --db {{db}} --keys {{keys}} {{args}}

# 读取某个会话的最近消息.
# 示例: just messages wxid_a1b2c3d4e5f60000 .tmp/db/message_1.db --limit 20
messages chat db *args:
    cargo run -q -p wxmsg-cli -- messages --chat {{chat}} --db {{db}} --keys {{keys}} {{args}}

# 从实时库 WAL 里找回被撤回的消息.
# 示例: just recalls .tmp/db/message_1.db --limit 20
recalls db *args:
    cargo run -q -p wxmsg-cli -- recalls --db {{db}} --keys {{keys}} {{args}}

# 从归档快照里找回被撤回的消息 (留多久就能回溯多久).
# 示例: just recalls-archive --snapshot 1791885630
recalls-archive *args:
    cargo run -q -p wxmsg-cli -- recalls --archive "$HOME/Library/Application Support/wxmsg/archive" {{args}}

# 取一次归档快照并裁剪.
snap *args:
    cargo run -q -p wxmsg-cli -- snap {{args}}

# 常驻监听并按间隔取归档快照, Ctrl-C 结束.
keep *args:
    cargo run -q -p wxmsg-cli -- keep {{args}}

# 导出会话为 Markdown.
# 示例: just export .tmp/db/message_1.db .tmp/out
export db out *args:
    cargo run -q -p wxmsg-cli -- export --db {{db}} --keys {{keys}} --out {{out}} {{args}}

# 抓取库密钥, 需要在有调试权限的终端里跑, 并扫一次码.
capture *args:
    cargo run -q -p wxmsg-cli -- capture {{args}}

# 启动界面 (用日常数据目录与平台日志位置).
gui:
    cargo run -q -p wxmsg-desktop

# 以隔离的数据目录与最详细的日志启动调试实例.
# 不触碰日常数据, 调试实例也能与正式实例同时运行.
debug:
    APP_DATA_DIR=target/wxmsg-debug APP_LOG_DIR=target/wxmsg-debug/logs RUST_LOG=wxmsg_desktop_lib=trace,wxmsg_core=debug cargo run -q -p wxmsg-desktop

# 打包出 .app 与 dmg 安装包.
dist:
    cargo tauri build

# 查看当前生效的配置, 以及各路径最终落在哪里.
show-config *args:
    cargo run -q -p wxmsg-cli -- config {{args}}

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
# 示例: just messages wxid_aaaaaaaaaaaaaa .tmp/db/message_1.db --limit 20
messages chat db *args:
    cargo run -q -p wxmsg-cli -- messages --chat {{chat}} --db {{db}} --keys {{keys}} {{args}}

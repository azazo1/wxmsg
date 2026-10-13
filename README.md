# wxmsg

微信聊天记录管理器, 面向 macOS 上的微信 4.x.

目标能力:

- **撤回监听与恢复**: 长期盯住本地存储, 撤回发生时把原文找回并归档
- **记录备份**: 加密库与附件的增量留存, 解密按需进行
- **记录导出**: 会话, 消息, 媒体导出为 HTML / JSON / Markdown
- **密钥管理**: 抓取, 校验, 历史留存
- **图形界面**: 浏览, 搜索, 导出

## 现状

**M1 已完成**: 用抓到的密钥直连 SQLCipher 加密库, 列出真实会话与消息, 不需要手工解页。

- 已确认的事实 (存储格式, 密钥路径, 撤回行为) 与架构, 里程碑见移交文档
- 界面框架已定: 保持 Tauri, 决策记录见移交文档第 11 节
- 进度与下一步见移交文档第 12 节

当前可用的命令:

```shell
# 列出某个消息分片中的会话, 按最近消息时间排序
wxmsg sessions --db <数据根>/db_storage/message/message_1.db \
  --keys secrets/all_keys.json --copy-to .tmp/db

# 读取某个会话的最近消息
wxmsg messages --chat <内部 wxid> --db .tmp/db/message_1.db \
  --keys secrets/all_keys.json --limit 20

# 把加密库重建为明文副本, 并与 SQLCipher 直连的结果逐表比对行数
wxmsg plain --db <数据根>/db_storage/message/message_1.db \
  --keys secrets/all_keys.json --copy-to .tmp/db --out .tmp/plain.db --verify

# 从 WAL 里找回被撤回 (被删除) 的消息
wxmsg recalls --db <数据根>/db_storage/message/message_1.db \
  --keys secrets/all_keys.json --copy-to .tmp/db --limit 20

# 持续留存: 每 5 分钟最多一次快照, 保留 7 天, 上限 8 GiB (只存密文)
wxmsg keep

# 只取一次快照, 便于交给 launchd 之类定时器调用
wxmsg snap --once

# 从归档快照里找回 (留多久就能回溯多久)
wxmsg recalls --archive ~/Library/Application\ Support/wxmsg/archive --limit 20

# 导出会话 (Markdown / JSON / CSV), 媒体文件一并复制到导出目录的 media/ 下
wxmsg export --db <数据根>/db_storage/message/message_1.db --out ./导出 --format md

# 导出时把撤回的消息按时间插回原处并标注
wxmsg export --db <数据根>/db_storage/message/message_1.db --out ./导出 --chat <wxid> --with-revoked
```

# 查看当前生效的配置, 以及各路径最终落在哪里
wxmsg config

# 把默认配置写到 ~/Library/Application Support/wxmsg/config.toml
wxmsg config --init
```

`plain` 的产物含完整聊天记录, 只用于排错与自检, 用完请删除。

`recalls` 不依赖任何事前归档: 被删消息所在页的旧版本只要还在 WAL 文件里就能找回。WAL 是
固定大小的环形文件, 越早的撤回越可能已经被覆盖, 所以要稳定还原需要让 `keep` 常驻。

`--copy-to` 会先把库连同 WAL 复制到指定目录再打开, 避免直接读写微信正在使用的文件。
密钥文件放在 `secrets/` 下, 不入库。

- [移交文档](docs/handover.md): 背景, 结论, 架构, 里程碑, 未决问题
- [原始台账](docs/ledger-rea-wechat-recall.md): 逐轮的实测记录与命令

## 技术栈

| 层 | 选择 | 理由 |
| --- | --- | --- |
| 核心 | Rust | 直接用 SQLCipher 打开加密库, 大文件性能, 常驻开销低 |
| 界面 | Tauri | 体积小, 系统 WebView, Web 技术适合聊天渲染与导出 |
| 密钥抓取 | Frida sidecar | 一次性动作, 复用已跑通的钩子脚本 |

## 目录结构

```
crates/core      解密/归档/撤回还原/密钥管理
crates/capture   一次性密钥抓取
crates/cli       命令行入口, 可被 launchd 调度
apps/desktop     Tauri 图形界面
sidecar           Frida 钩子脚本
docs             移交文档与台账
```

## 阅读顺序

1. [移交文档](docs/handover.md) 的 "已确认的事实" 一节, 了解微信的存储格式, 密钥路径与撤回行为
2. 同文档的 "架构" 与 "里程碑" 一节, 了解从哪一行代码开始写
3. 需要细节时查 [原始台账](docs/ledger-rea-wechat-recall.md)

## 注意

本项目只处理**本机本账号**的本地数据, 不做任何网络上传. 使用前请阅读移交文档中的风险与合规一节.

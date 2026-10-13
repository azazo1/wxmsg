# wxmsg

微信聊天记录管理器, 面向 macOS 上的微信 4.x.

目标能力:

- **撤回恢复**: 把被撤回 (被删除) 的消息原文找回, 并持续归档以便回溯
- **记录备份**: 加密库与 WAL 的增量留存, 解密按需进行
- **记录导出**: 会话, 消息, 媒体导出为 Markdown / JSON / CSV
- **密钥管理**: 一次性抓取, 按内容校验, 带时间留存
- **图形界面**: 浏览会话与消息, 查看撤回记录

## 现状

七个里程碑都已落地, 各自的验证情况见移交文档第 12 节。

```shell
just build          # 构建
just test           # 单元测试
just lint           # 静态检查, 警告视为错误
just check-private  # 提交前扫描个人信息与密钥
```

### 图形界面

```shell
just gui      # 用日常数据目录启动界面
just debug    # 隔离数据目录与最详细日志, 可与正式实例同时运行
just dist     # 打包出 .app 与 dmg
```

关闭窗口只是隐藏到托盘; 真正退出走托盘右键菜单或状态栏的退出按钮。界面的详细说明见
[apps/desktop/README.md](apps/desktop/README.md).

### 命令行

```shell
# 列出某个消息分片中的会话, 按最近消息时间排序
wxmsg sessions --db <数据根>/db_storage/message/message_1.db \
  --keys secrets/all_keys.json --copy-to .tmp/db

# 读取某个会话的最近消息
wxmsg messages --chat <内部 wxid> --db .tmp/db/message_1.db \
  --keys secrets/all_keys.json --limit 20

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

# 查看当前生效的配置; --init 写出默认配置
wxmsg config

# 抓取库密钥 (需要扫一次码, 且要有调试权限的终端)
wxmsg capture
```

`recalls` 不依赖任何事前归档: 被删消息所在页的旧版本只要还在 WAL 文件里就能找回。WAL 是固定
大小的环形文件, 越早的撤回越可能已经被覆盖, 所以要稳定还原需要让 `keep` 常驻。

`--copy-to` 会把库连同 WAL 复制到指定目录再打开, 避免直接读写微信正在使用的文件。
密钥文件放在 `secrets/` 下, 不入库。

## 技术栈

| 层 | 选择 | 理由 |
| --- | --- | --- |
| 核心 | Rust | 直接用 SQLCipher 打开加密库, 也能自己按页解密；大文件性能好, 常驻开销低 |
| 界面 | Tauri | 体积小, 用系统 WebView；前端是一份静态页面, 不需要 Node 工具链 |
| 密钥抓取 | Frida sidecar | 一次性动作, 复用已验证的钩子脚本；依赖由 uv 管 |

## 目录结构

```
crates/core            解密, 归档, 撤回还原, 密钥, 导出
crates/cli             命令行入口 (含密钥抓取), 可被 launchd 调度
apps/desktop           Tauri 图形界面
sidecar                Frida 钩子脚本与抓取脚本
docs                   移交文档与台账
scripts                静态检查等辅助脚本
```

## 阅读顺序

1. [移交文档](docs/handover.md) 的 "已确认的事实" 一节, 了解微信的存储格式, 密钥路径与撤回行为
2. 同文档的 "架构" 与 "里程碑" 一节, 了解代码的组织方式与已完成的部分
3. 需要细节时查 [原始台账](docs/ledger-rea-wechat-recall.md)

## 注意

本项目只处理**本机本账号**的本地数据, 不做任何网络上传. 使用前请阅读移交文档中的风险与合规一节.

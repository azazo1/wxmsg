# 微信撤回链与内容恢复: 接续清单

状态: 等待宿主应用获得"完全磁盘访问权限", 之后继续抓密钥.

## 目标

1. 回答"撤回后本机文字内容是否还在" (需要解密数据库)
2. 检查 `-wal` 与 `*.material` 里是否留有撤回前的旧版本页面
3. 主目标: 撤回的完整处理链 (通知 -> 本地状态改写 -> 界面更新)

## 已完成

| 项 | 位置 / 结果 |
| --- | --- |
| 调查台账 | `/tmp/rea-wechat-recall/FINDINGS.md` (第 1 到 10 轮) |
| 方法论文档 | 已推送到 `azazo1/rea` 的 `docs/large-binary-scoped-analysis.md` |
| 有界分析快照 | `wx-bounded2-snapshot.json`, `wx-bounded3-snapshot.json` |
| 存储快照 | `store-baseline.json`, `store-sent.json`, `store-out.json`, `store-revoked.json` |
| 快照工具 | `snapshot_store.py` |
| 开源项目克隆 | `vendor/wechat4-macos-frida-export` |
| 依赖 | 已 `uv sync` (frida 17.18.0, pycryptodome, zstandard) |
| 调试副本 | 已构建 `.runtime/WeChat-SIP.app`, 注入成功, 官方 app 未改动 |

## 已确认的关键事实

- 撤回会**删除**明文文件副本 (`msg/file/<年-月>/<原名>`), 无回收站, 容器内无第二份
- `db_storage` 下**所有**库都加密 (含 `message_fts.db` 全文索引)
- 加密参数: AES-256-CBC, 原始 32 字节密钥不做 PBKDF2, HMAC-SHA512,
  页大小 4096, 保留区 80, 盐 = 文件前 16 字节, 每个库独立密钥与盐
- 会话附件目录名 = `md5(用户名)`; 消息表名 = `Msg_<md5(用户名)>`
- 本机无备份: Time Machine 未配置, 本地快照只有系统更新用的, `Backup` 只有 44 KB 空壳

## 下一步: 授权之后执行

1. 确认官方微信已退出
2. 抓密钥:

```shell
cd /tmp/rea-wechat-recall/vendor/wechat4-macos-frida-export
.venv/bin/python -m wxexport.keys.hunt --restart
```

3. 调试副本启动后**扫码登录一次**, 然后**手动打开会话并滚动历史**
   (密钥只在微信真正解密某个库时出现). 重点打开:
   - 与 `alias_a` 的聊天 (发过 `demo.txt` 并撤回)
   - 文件传输助手
4. 连续 45 秒无新密钥会自动停止, 然后:

```shell
.venv/bin/python -m wxexport.keys.map
.venv/bin/python -m wxexport.decrypt
```

5. 解密产物在 `.runtime` 下的 `decrypted/`, 用 `sqlite3` 直接查:
   - 找到 `Msg_<md5(用户名)>` 表
   - 检查那条被撤回消息的 `message_content` 是被清空还是只打了标记
   - 对照 `-wal` 与 `*.material`, 寻找撤回前的旧版本页面

## 授权方式

系统设置 -> 隐私与安全性 -> 完全磁盘访问权限 -> 加入运行 agent 的应用
(DeepSeek Harness 或所在终端) -> 完全退出并重开该应用.

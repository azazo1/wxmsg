# wxmsg 移交文档

本文档把微信 4.x (macOS) 聊天记录管理器的前期调研结论, 技术选型与实现路线完整交接。
所有结论都来自真机实测, 逐条证据见 [原始台账](ledger-rea-wechat-recall.md)。

## 1. 项目定位与边界

**做什么**

- 撤回监听与恢复: 常驻盯住本地存储, 撤回发生时把原文找回并归档
- 备份: 加密库与附件的增量留存, 解密按需进行
- 导出: 会话, 消息, 媒体导出为 HTML / JSON / Markdown
- 密钥管理: 抓取, 校验, 历史留存
- 图形界面: 浏览, 搜索, 导出

**不做什么**

- 不修改官方微信客户端
- 不向任何服务器上传数据
- 不处理他人账号数据, 只处理本机当前登录账号的本地文件

## 2. 已验证的事实

### 2.1 环境与目标版本

| 项 | 值 |
| --- | --- |
| 系统 | macOS 15.6 (24G84), Apple M1, 16 GB |
| 微信 | 4.1.7 (build 34371), `/Applications/WeChat.app` |
| 主二进制 SHA-256 | `764966cdaaf945bc8b23968bb7b3dca3cdc4067e2891a38e28c7556788e0682c` |
| arm64 slice SHA-256 | `f7f9b8044c911e674d5a7c8b377f410a2784c07d67d80b5bce188ed221b199be` |
| 主二进制函数数 | 328,989 |

### 2.2 本地存储结构

数据根: `~/Library/Containers/com.tencent.xinWeChat/Data/Documents/xwechat_files/`

```
all_users/                     全局配置与登录态
  login/<wxid>/key_info.db     明文 SQLite 的 "密钥信息" 库 (内容封装)
<wxid>_<suffix>/               账号目录
  db_storage/                  全部加密的 SQLite 库
    message/message_0.db       早期消息分片
    message/message_1.db       近期消息分片
    message/message_fts.db     全文索引
    message/message_resource.db
    contact/contact.db         联系人 (含 alias 别名与 username 映射)
    session/session.db
  msg/                         附件明文, 结构为 msg/<类别>/<yyyy-mm>/<原名>
  config/  resource/  cache/  business/
```

要点:

- 账号目录名里的 `wxid_...` 是**内部 username**, 与微信号别名不同
- 附件目录名等于 `md5(username)`, 可用 `md5("filehelper") = 9e20f478899dc29eb19741386f9343c8` 自检
- `db_storage` 下所有 `*.db` 都是加密的 (文件头不是 `SQLite format 3`), 含 `kvdb` / `MMKV`
- 每个库为**独立的 32 字节原始密钥**, 库与密钥一一对应

### 2.3 加密格式与参数

SQLCipher 4 标准格式, 已实测可稳定解密:

| 参数 | 值 |
| --- | --- |
| 算法 | AES-256-CBC |
| 页大小 | 4096 |
| 预留区 | 末 80 字节 = IV 16 字节 + HMAC 64 字节 |
| 密钥 | 每库独立 32 字节原始密钥 (直接作为 AES key, 不经 PBKDF2) |
| 库盐值 | 页 1 的前 16 字节 |
| HMAC | HMAC-SHA512, 覆盖 密文 + IV + 页号小端 4 字节 (页 1 的盐值**不**参与) |
| mac key | `PBKDF2-HMAC-SHA512(enc_key, salt XOR 0x3a, 2 轮, 32 字节)`, 轮数取 SQLCipher 的 `fast_kdf_iter` |

解密步骤: 取页的最后 80 字节中的前 16 字节作 IV, 用密钥解 `page[0..4096-80]`; 页 1 的密文从
偏移 16 开始 (前 16 字节是盐值), 且明文的前 16 字节由 SQLCipher 固定补回为
`SQLite format 3\0`, 不参与加密。

以上参数在实现时重新核对过 SQLCipher 源码 (bundled-sqlcipher 里的 `sqlite3.c`, 函数
`sqlcipher_page_hmac` 与 `sqlcipher_cipher_ctx_key_derive`), 并用真实库的 5282 页做过
逐表行数比对。**早期台账里"页 1 额外覆盖前 16 字节"的说法是错的**, 以本节为准。

### 2.4 密钥从哪里来

这是整个项目最关键的一条边界, 结论如下。

**磁盘上没有明文密钥**

- 容器内 923 个文件里, 明文 SQLite 库**只有一个**: `all_users/login/<wxid>/key_info.db`
- 该库的 `LoginKeyInfoTable(user_name_md5, key_md5, key_info_md5, key_info_data)` 有 150 行, 每行 180 字节
- 已验证 `key_info_md5 == md5(key_info_data)`, 且 `user_name_md5 == md5(wxid)`
- 但**这 150 条与 19 个已知库密钥没有任何直接或哈希对应关系**, 内容是封装的
- 19 个已知库密钥在 923 个文件中**明文出现 0 次**; 钥匙串中也没有微信条目

**密钥只在登录态的进程内出现**

- 抓取钩子装在 CommonCrypto 的 `CCCryptorCreate` / `CCCrypt` / `CCCryptorCreateWithMode` / `CCKeyDerivationPBKDF` 上
- **未登录时抓不到任何密钥**: 实测未登录状态下钩子已装载, 但加密库文件自始至终未被打开, 无任何加解密调用
- **完全断网时连登录二维码都取不到** (二维码内容来自服务端), 应用根本走不到打开数据库那一步
- 结论: 无法用"纯读本地文件 + 推导"的方式复刻密钥路径; 必须在**登录状态的进程内**、于加解密瞬间截获

**抓取方式 (已跑通)**

1. 复制 `/Applications/WeChat.app` 到临时目录; SIP 保持开启, 官方 App 不动
2. 改副本 `CFBundleIdentifier` (避免与官方冲突)
3. 删除带身份的 entitlement, 关键是 `com.apple.developer.team-identifier`
   (ad-hoc 签名却声明腾讯 Team ID 会被 AMFI 拒绝, 表现为 `Launchd job spawn failed`, errno 153)
4. 保留 `com.apple.security.cs.disable-library-validation`, 否则 Gadget 无法加载
5. 深度 ad-hoc 重签 (`codesign -f -s -`), 覆盖嵌套的 framework 与 helper
6. 注入 Frida Gadget: 增加指向 `@executable_path/../Frameworks/FridaGadget.dylib` 的
   `LC_LOAD_DYLIB`, 并放置配置 (监听 `127.0.0.1:27042`, `on_load: wait`)
7. 复制账号数据到副本 (只需 `db_storage` + `config` + `resource` + `all_users`;
   GB 级的 `msg` / `cache` / `business` 用空目录占位即可)
   - 必须用普通复制, `clonefile` 对带数据保险箱保护的文件会被拒绝
8. 以重定向的家目录启动副本 (见 2.6)
9. Frida 客户端连上 `127.0.0.1:27042` 装载钩子; 用户扫码登录一次
10. 密钥随各库被打开而流入; 用 HMAC 校验页 1 即可把密钥与库对上

**实现期必须注意的两个坑**

- `Interceptor.attach` 只能改返回值, **不能阻止** `connect` 真的建连; 要真正阻断必须用
  `Interceptor.replace` (+ `NativeCallback` + 写 `__error()`)
- 控制端要打印钩子上报的消息, 否则会被静默丢弃; 我们当初就因为只放行
  `KEY32` / `ARMED` / `MISS` / `PBKDF` 前缀而白查了一轮

### 2.5 撤回链路与可恢复性

**观察到的事实**

- 撤回后**原消息行不存在**, 同表出现一条 `local_type = 10000` 的系统消息:

  ```xml
  <sysmsg type="revokemsg"><revokemsg>
    <content>"某人" 撤回了一条消息</content>
    <revoketime>0</revoketime>
  </revokemsg></sysmsg>
  ```

- 可据此区分方向: 对方撤回时 `content` 是 `"<备注>" 撤回了一条消息` 且 `revoketime` 为 0;
  自己撤回时 `content` 是 `你撤回了一条消息` 且 `revoketime` 为非 0 (等于撤回时刻)
- 撤回会改写 `message/message_*.db`, `message_fts.db`, `message_resource.db`, `session.db`
  以及它们的 `-wal` / `-shm`; 库文件大小不变 (页内改写)
- `msg/` 下的明文附件**会被删除** (实测 `msg/file/2026-10/demo.txt` 消失, 无回收站副本)
- 每库另有 `-first.material` / `-last.material` / `-incremental.material` (各 18 页),
  内容为**模式页** (建表语句), 不含消息行

**可恢复性 (已实测)**

被删内容仍留在 `message_*.db-wal` 的**被取代帧**里。实测从 WAL 中完整还原出一条被撤回的
文件消息:

```xml
<msg><appmsg><title>demo.txt</title><type>6</type>
<appattach><totallen>5</totallen><attachid>@cdn_...</attachid>
<cdnattachurl>305f020100044b...</cdnattachurl></appattach></appmsg></msg>
```

即文件名, 大小, CDN 引用全部还原。做法: 逐帧读取 WAL, 用页号分组, 对同一页保留历史版本,
再对页内 zstd 流解压。

**时窗限制 (重要)**

实现阶段重新读了这个文件, 结论比早期更细:

- WAL 是固定 4 MB 的**环形**文件, 里面往往同时躺着**好几代**帧: 当前代 (盐值与文件头一致)
  与其它已被检查点冲刷过的旧代。旧代的文件头早已被覆盖, 但其帧字节仍在原处
- 因此**不是整份 WAL 只剩下很少历史**, 而是能一次性看到若干代。实测 `message_1.db` 的 WAL
  里有 9 代共 1018 帧, `message_0.db` 同样 1018 帧
- 但覆盖是**按字节**发生的: 新帧不断写入, 旧代的字节迟早被冲掉。实测 `message_1.db` 的
  WAL 里当时只剩 295 个不同的页, 上午那次撤回所在的页 (5063, 5231) **已经不在其中**
- 结论不变: 事后去挖只能捡到剩下的, 要稳定还原必须**持续留存 WAL 快照**

**尚未定论的一处**

撤回究竟是"删除原行 + 插入通知行", 还是"把原行原地改写为通知"。
两种解释都能说明"原行消失"与"旧内容留在 WAL"。
建表语句为 `local_id INTEGER PRIMARY KEY AUTOINCREMENT` (编号不复用), 但两次实测都没观察到
编号空缺, 二者存在矛盾。**做一次受控实验即可定论**: 发一条文字消息, 记下行号, 撤回, 再查行号
与 `sqlite_sequence`。

### 2.6 一次性抓取的工作形态

**关键发现: 重定向家目录即可让整个副本落在临时目录**

微信自行按 `家目录 + Library/Containers/<bundle-id>/Data` 拼接数据根 (子进程还带着
`--wechat-files-path=` 参数)。因此:

```shell
env HOME=<tmp>/home CFFIXED_USER_HOME=<tmp>/home \
  <tmp>/app/WeChat.app/Contents/MacOS/WeChat
```

实测结果:

| 检查项 | 结果 |
| --- | --- |
| 微信数据根 | 落在 `<tmp>/home/Library/Containers/<bundle-id>/Data` |
| 偏好与崩溃报告 | 落在 `<tmp>/home/Library/...` |
| 真实 `~/Library/Containers/<副本 id>` | 0 个新写入 |
| `~/Library/Preferences` / `Caches` 中微信相关条目 | 0 个 |

也就是说: **官方微信零改动, `~/Library` 无我方产生的写入, 临时目录删掉即无痕。**
唯一需要用户动手的是扫一次码。

注意: 本机 `sandbox-exec` 已失效 (档案能应用但不产生任何限制), 不能用它做按进程网络隔离;
按进程/按用户阻断网络需要 root 或第三方防火墙, 均不适合本项目。

## 3. 数据模型速查

| 项 | 规则 |
| --- | --- |
| 消息表名 | `Msg_<md5(username)>`, username 为内部 wxid, 不是别名 |
| 会话号映射 | `Name2Id(rowid, user_name)`, **每个分片各有自己的编号空间** |
| 消息正文 | `message_content = "<发送者 wxid>:\n<载荷>"` |
| 真实类型 | `local_type & 0xFFFFFFFF` |
| 压缩标记 | `WCDB_CT_message_content = 4` 时为 zstd, 帧头 `28 b5 2f fd` |
| 撤回通知 | `local_type = 10000`, 正文为 `revokemsg` 的 sysmsg XML |
| 表结构 | `local_id, server_id, local_type, sort_seq, real_sender_id, create_time, status, upload_status, download_status, server_seq, origin_source, source, message_content, compress_content, packed_info_data, WCDB_CT_message_content, WCDB_CT_source` |
| 联系人 | `contact.db` 的 `contact(username, alias, remark, nick_name)` |
| 发送者 | **只能看 `real_sender_id`**: 它是本分片 `Name2Id` 的行号 |
| 资源索引 | `message_resource.db` 的 `MessageResourceInfo` / `MessageResourceDetail` 只有体积与状态, **没有文件路径** |

**发送者不能从正文里猜** (实测踩过):

正文里的 `发送者:` 前缀只在一部分消息里存在, **一对一聊天根本没有这个前缀**。
`real_sender_id` 才是可靠来源, 换成 wxid 之后再去 `contact` 表查显示名。
本用户自己发的消息, `real_sender_id` 指向 `Name2Id` 里代表本人的那一行。

**媒体文件的位置规律** (导出时用):

| 类型 | 位置 |
| --- | --- |
| 文件 | `msg/file/<年-月>/<原文件名>` |
| 图片缩略图 | `msg/attach/<会话 wxid 的 md5>/<年-月>/Img/<编号>_<时间>_t.dat` |
| 视频封面 | `msg/video/<年-月>/<md5>_thumb.jpg` |

图片那一条要留个心眼: `<编号>` **不总是消息行号**, 有些文件用的是与消息里 `md5` 无关的另一串
哈希。因此图片只在三种候选都试过之后才认定, 试不到就只写 `[图片]`, 不拿同目录的其它文件顶替
(宁可缺, 不可错)。文件附件按"名字加月份"定位, 命中率很高。

**直接解页时必须知道的几件事** (SQLCipher 接口读不到这些):

- `local_id` 是 `INTEGER PRIMARY KEY`, 即 rowid 的别名。SQLite 在**记录里给这一列存的是
  NULL**, 真实值只能从单元的行号取; 按记录取值会全表读成空
- 一页的可用空间 = 页大小 - 保留区 = 4016 字节。叶页单元能整段放下的载荷上限是
  `可用空间 - 35`, 超出后按 `minLeaf + (载荷 - minLeaf) % (可用空间 - 4)` 取局部分段,
  且局部段最后 4 字节是溢出页号。解这类行要么跟随溢出链, 要么放弃该行
- WAL 的校验和覆盖范围是 密文加 IV 再加页号小端 4 字节; 旧代的第一帧因为链的起点
  (旧文件头) 已消失, 只能信任帧里记录的校验和

## 4. 技术选型

**Rust 核心 + Tauri 界面**, 理由:

1. **可直接打开加密库**。`rusqlite` 带 `bundled-sqlcipher-vendored-openssl` 特性, 可以直接
   `PRAGMA key = "x'<原始密钥>'"` 打开微信的库, 不必自己逐页解密 (前期验证用的手工解密
   400 MB 要一分多钟, 还要自己拼 WAL)。这是最大的架构收益
2. **常驻监听开销低**。撤回监听要长期跑, 小二进制, 低内存; 也方便做成 launchd 守护,
   界面关闭时照常工作
3. **数据量大**。账号 `msg` 目录 7.5 GB, 消息库 172 MB + 20 MB, 全文索引 46 MB,
   备份/导出/搜索都要反复过这些数据
4. **分发与前端**。Tauri 用系统 WebView, 打包 10 到 20 MB; 聊天渲染, 搜索高亮,
   HTML 导出预览都适合用 Web 技术

密钥抓取用**独立 sidecar** (Frida CLI 或 frida-python), 因为它是偶发的一次性动作,
Rust 端不需要 Frida 绑定:

```shell
frida -H 127.0.0.1:27042 -l sidecar/hook.js
```

备选方案 (供权衡): Python + PySide6 出成果最快, 能直接复用已验证的解密与钩子代码,
代价是打包 150 MB 起, 常驻内存高, 且没有 SQLCipher 直连。

## 5. 架构与模块职责

```
crates/core      纯逻辑, 不依赖界面
  keys            密钥存储, HMAC 校验, 密钥历史 (带时间戳, 支持轮换)
  store           SQLCipher 直连打开, 会话与消息读取
  archive         密文留存 (库 + WAL + material), 解密按需
  recall          撤回检测 (revokemsg), WAL 历史帧还原, 快照差分
  export          HTML / JSON / Markdown 导出, 媒体定位
crates/capture   一次性密钥抓取
  copy            复制 App, 改 bundle id, 去身份 entitlement, 深度重签
  gadget          注入 Frida Gadget
  data            复制账号数据到临时家目录
  launch          重定向 HOME 启动, 连接 Gadget, 收集密钥
  cleanup         清理临时目录
crates/cli       命令行入口 (capture / keep / snap / recalls / export)
apps/desktop     Tauri 界面: 会话列表, 消息浏览, 搜索, 撤回还原视图, 导出向导
sidecar/hook.js  Frida 钩子脚本
```

**分层原则**

- 核心与界面解耦, 界面只是壳
- 归档**先存密文**, 解密按需; 换密钥不会让旧快照作废
- 库与密钥的对应关系**用 HMAC 校验自动判定**, 不依赖人工登记

## 6. 里程碑

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| M1 | `core::store` 用已知密钥直连打开 `message_*.db`, 列出会话与最近消息 | 能读出与官方界面一致的会话列表 |
| M2 | `core::recall` 从 WAL 历史帧还原一条真实撤回的原文 | 还原出被撤回消息的完整正文 |
| M3 | `core::archive` 增量留存库与 WAL, 解密按需 | 留存期间发生的撤回可事后还原 |
| M4 | `crates/capture` 一次性抓取密钥 (重定向 HOME, 零 `~/Library` 写入) | 扫码一次拿到全部库密钥且无残留 |
| M5 | `crates/cli` 四个子命令打通 | 纯命令行可完成监听与还原 |
| M6 | `apps/desktop` Tauri 界面 | 浏览/搜索/查看撤回/导出 |
| M7 | 导出器与媒体定位 | HTML 导出可在浏览器完整回看 |

建议从 M1 开始: 它直接验证"SQLCipher 直连"这个核心假设, 失败则需回到手工解密方案。
**M1 已于 2026-10-13 验证通过**, 见 12 节。

## 7. 未决问题

1. 密钥轮换的触发条件与检测方式 (已知: 更新, 切号, 记录迁移; 检测: HMAC 校验首页)
2. 撤回是删行还是原地改写的定论 (见 2.5, 需一次受控实验)
3. 归档策略参数: 留存频率, 保留层数, 增量去重方式
4. `message_fts.db` 能否作为撤回内容的第二条恢复路径 (初步未命中, 编号空间按分片不同)
5. 分片规则: 什么条件下写入 `message_0.db` 还是 `message_1.db`
6. 并发安全: 微信正在写 WAL 时读取的一致性保证
7. Windows 版微信的对应方案 (加密参数与密钥路径不同, 需另做调研)

## 8. 风险与合规

- **账号风险**: 扫码登录副本可能被微信视为新设备, 存在账号受限的可能; 官方端会因此下线
- **版本风险**: 微信 4.1.10 的更新曾让既有解密工具集体失效 (密钥不再以字符串驻留内存,
  SQLCipher 符号被剥离); 本项目同样可能被后续更新打断
- **数据风险**: 归档含完整聊天记录, 属高敏感数据; 密钥文件等同库本身, 必须 0600 权限,
  绝不入库, 绝不上传
- **合规**: 只处理本机本账号数据; 还原他人撤回内容涉及对方预期, 使用者需自行判断

## 9. 移交资产清单

| 资产 | 位置 | 说明 |
| --- | --- | --- |
| 原始台账 | `docs/ledger-rea-wechat-recall.md` | 逐轮实测记录, 含命令与输出摘要 |
| Frida 钩子 | `sidecar/hook.js` | 已验证可用, 含 CommonCrypto 钩子与断网开关 |
| 本文件 | `docs/handover.md` | 结论, 架构, 路线 |
| 库密钥 | `secrets/all_keys.json` | 抓到的 19 个库密钥, 权限 0600, 已被 `.gitignore` 排除 |
| 参考材料 | `.references/` | 上游项目源码快照与本地补丁 diff, REA 分析脚本, 已被 `.gitignore` 排除 |

已于 2026-10-13 清理的临时资产 (清理前已把必要内容复制进本项目):

- 一次性抓取用的工作区 (当时在 `/tmp/rea-wechat-recall`) 与重定向家目录的测试容器
- 已解密的库副本, 调试副本容器 (含真实聊天记录明文) 与注入后的 `WeChat-SIP.app`
- 37 个 Ghidra 临时工程目录 (5.5 GB)

密钥与机器绑定, 换机或轮换后失效, 那时按 2.4 节重跑一次抓取即可。

## 10. 环境与工具要求

| 工具 | 版本 | 用途 |
| --- | --- | --- |
| Rust | 1.99 | 核心与界面 |
| Node.js / pnpm | 26.x / 最新 | Tauri 前端 |
| Xcode 命令行工具 | 已装 | 编译, `codesign`, `plutil` |
| Frida | 17.x | 密钥抓取 (Python 绑定或 CLI) |
| just | 1.58 | 任务编排 |

权限要求: 读取 `~/Library/Containers/com.tencent.xinWeChat` 需要"完全磁盘访问权限";
抓取时官方微信需先退出 (同一账号单设备在线的服务端策略)。

## 11. 界面选型的决策记录

界面框架已定: **保持 Tauri**。评估过 egui, 记录如下, 便于后续复评。

### egui 能省多少

| 项 | Tauri | egui (glow 后端) |
| --- | --- | --- |
| 空闲内存 | 跨 3 到 4 个进程, 合计 150 到 300 MB | 单进程 40 到 90 MB |
| 应用包体积 | 10 到 20 MB | 4 到 8 MB |
| target 目录与编译时间 | Rust 侧要编 wry / tao / WebKit 绑定, 前端还要打包 | 更轻; 但若用 wgpu 后端则与 Tauri 相当 |

内存的差距来自 WKWebView 的辅助进程 (`WebKit.WebContent` 等), 不是 Rust 进程本身。
Tauri 在 macOS 上不打包 Chromium, 因此体积本来就不大。

### 没换 egui 的三个原因

1. **中文字体**: egui 默认字体不含 CJK, 要么打包字体 (加 5 到 15 MB, 吃掉体积优势),
   要么运行时加载系统苹方 (0 MB 但失去跨平台)。Tauri 走系统 WebView, 天然零成本
2. **界面工作量**: 聊天记录是异构内容 (文字, 图片, 表情, 引用, 合并转发, 文件卡片),
   HTML/CSS 里是常规活, egui 里要自己写气泡布局, 虚拟滚动, 文本选中, 图片缓存
3. **导出预览**: Tauri 直接把 HTML 交给 WebView; egui 里要么嵌 WKWebView (内存又回来),
   要么交给外部浏览器

只做 macOS 的话还有第三条路: SwiftUI, 体积 1 到 3 MB, 中文走系统字体, 需要预览时再嵌
WKWebView。

### 这个决定为什么可以晚做

`crates/core` 与 `crates/cli` 完全不依赖界面框架, M1 到 M5 与界面无关。
若 M6 之前需求变化 (例如必须极致省内存), 换界面的代价仅限于 `apps/desktop`。

## 12. 当前进度与文件清单

**M1 已完成并验证** (2026-10-13): `core::store` 用抓到的密钥直连真实的 `message_1.db`,
不需要任何手工解页。

### 验证结果

```shell
wxmsg sessions --db "<数据根>/db_storage/message/message_1.db" \
  --keys secrets/all_keys.json --copy-to .tmp/db --limit 15
```

| 检查项 | 结果 |
| --- | --- |
| SQLCipher 直连 | 成功, `PRAGMA key = "x'<32 字节原始密钥>'"` 即可打开 |
| 会话数 (`Name2Id`) | 531 条, 其中 50 条有消息表 |
| 消息总数 | 34,086 |
| 会话排序 | 按最近消息时间倒序, 与微信会话列表一致 |
| 正文解码 | zstd 压缩的 appmsg XML 正确解出 |
| 撤回识别 | 命中 `local_id = 4707` (08:44:11), 与 2.5 节的取证结论一致 |

### 骨架暴露出的问题

移交时那份骨架**未编译**, 首次构建与验证发现并修掉了五处:

| 问题 | 修法 |
| --- | --- |
| `serde` 未开 `derive` 特性, 派生宏找不到 | 加 `features = ["derive"]` |
| md-5 0.11 的 `finalize()` 返回定长数组, 不再实现 `LowerHex` | 新增 `core::hex`, 统一编解码 |
| `message_content` 的存储类型不固定 (TEXT 或 zstd BLOB), 直接按 `Vec<u8>` 取会在 TEXT 上报类型错误 | `read_content` 按 SQLite 动态类型分派 |
| 库被 `--copy-to` 复制后路径层级改变, 密钥的路径尾部匹配失效 | `KeyStore::get` 增加文件名兜底匹配, 同名库出现多个时拒绝并提示 |
| 会话条目没有最近消息时间, 列表顺序无法与界面对齐 | `ChatEntry` 增加 `last_message_time`, 与 `count(*)` 在同一次扫描中取出 |

### 文件清单

| 路径 | 状态 | 说明 |
| --- | --- | --- |
| `README.md` | 完成 | 项目说明与阅读顺序 |
| `docs/handover.md` | 完成 | 本文件 |
| `docs/ledger-rea-wechat-recall.md` | 完成 | 前期逐轮实测台账, 676 行 |
| `docs/runbook-resume-checklist.md` | 完成 | 当时的续跑清单, 仅供参考 |
| `sidecar/hook.js` | 完成 | 已验证可用的 Frida 钩子 |
| `sidecar/README.md` | 完成 | 钩子说明与出处 |
| `justfile` | 完成 | 常用任务: 构建, 测试, 静态检查, 隐私扫描 |
| `scripts/check-private.py` | 完成 | 提交前扫描个人信息与密钥; 专属敏感词在 `secrets/private-terms.txt` |
| `LICENSE` | 完成 | MIT, 与 `Cargo.toml` 的 license 字段一致 |
| `crates/core/**` | **已编译, 已验证** | `hex` `error` `keys` `store` `chat` `message` `crypto` `verify` `wal` `btree` `image` `recall` `archive` |
| `crates/cli/**` | **已编译, 已验证** | `main` `cli` `paths` `commands/{sessions,messages,plain,recalls,snap,keep,archive,table}` |
| `secrets/` | 本地 | 19 个库密钥, 权限 0600, 已被 `.gitignore` 排除 |
| `.references/` | 本地 | 上游项目源码与本地补丁 diff, REA 分析脚本, 已被 `.gitignore` 排除 |
| `apps/desktop` | 空 | 未开始, 到 M6 再做 |

依赖已由 `cargo add` 写入两个 `Cargo.toml`。首次构建要编译 OpenSSL 与 SQLCipher, 约数分钟;
之后的增量构建是秒级。

### M2 完成情况

M2 需要的四个模块都已落地:

| 模块 | 职责 |
| --- | --- |
| `core::crypto` | SQLCipher 页解密: AES-256-CBC 加 HMAC-SHA512 校验; `plaintext` 能把整库重建为明文 |
| `core::wal` | WAL 头与帧解析, 逐帧校验和链; **按盐值分出多代**, 旧代同样可用 |
| `core::btree` | 页头, 叶页单元, 溢出判定, varint 与记录解码; 内部页遍历收集叶页 |
| `core::recall` | 定位每张表的叶页, 把主库与 WAL 各代的每一个页版本解成行, 与当前行集合做差分 |

两个自检与外测结果:

| 检查 | 结果 |
| --- | --- |
| `plain --verify` (自己的解密对比 SQLCipher 直连) | 5286 页, 54 张表行数完全一致 |
| `recalls` 对 `message_1.db` 的解码量 | 解出 35,490 个单元, 行号与 SQLite 的当前行集**零误报** |
| `recalls` 对 `message_0.db` (180 MB, 123 张表) | 解出 241,192 个单元, 找回 1 条已从表中消失的消息 |
| `recalls` 对 `message_0.db` 的耗时 | 约 33 秒 (主要是 39,213 个叶页的遍历) |

找回的那一条是 2026-08-27 的"拍一拍"记录 (`local_id = 665`, 类型 49), 它已经不在当前表里,
但内容从 WAL 第 9 代第 121 帧完整解出, 与当前表对照后判定为已删除行。也就是说**删除行还原的
通路成立**, 只是当前这份 WAL 里已经找不到上午那次撤回所涉及的页。

### M3 完成情况

M3 要解决的是 M2 的软肋: WAL 是环形文件, 事后去挖只能捡到剩下的。归档把密文按时间留下来。

| 模块 | 职责 |
| --- | --- |
| `core::archive::chunk` | 内容寻址的块存储, 默认 1 MiB 一块, 按 sha256 去重, 读回时校验 |
| `core::archive` | 快照清单 (`version` / `created_at` / 逐文件块序列), 增量判定, 还原, 保留策略与块回收 |
| `core::keys::KeyHistory` | 密钥按时间累积保存; 用 HMAC 自动判定某把密钥能否开某个库 |
| `wxmsg snap` / `wxmsg keep` | 取一次快照 / 常驻监听 (FSEvents 加去抖与节流) |
| `wxmsg recalls --archive` | 从快照还原库与 WAL 后再找回, 用完即删明文副本 |

实测 (对真实账号目录, 归档放在工作区内):

| 检查 | 结果 |
| --- | --- |
| 首次快照 (79 个文件, 404.7 MB) | 12.1 秒 |
| 第二次快照 (无变化) | 0.8 秒, 归档体积**零增长** |
| 还原校验 | 还原出来的每个库与 WAL 的 sha256 与清单**逐字节一致** |
| `recalls --archive` | 从快照里找回同一条已删消息, 并自动清理明文副本, 全程 67 秒 |

默认参数 (都可在命令行覆盖): 每 300 秒最多一次快照, 变化后静置 3 秒才动手, 保留 7 天,
总量上限 8 GiB, 到顶自动丢最旧的快照并回收其块。归档只含密文, 因此不含明文聊天记录。

这一轮还修掉两个只看实时库时暴露不出来的问题:

- 页遍历必须把**当前代 WAL 叠加到主库之上**。检查点前后主库文件本身还是旧状态, 只看文件会
  沿着过期的 B 树指针走到已被回收的页
- 旧代的同一个页号当时可能装着索引页或空闲页, 解码时不能假定它一定是叶页

### M5 完成情况

配置与命令行的分工: **命令行 > 配置文件 > 内置默认**。配置文件是 TOML, 默认在
`~/Library/Application Support/wxmsg/config.toml`, 每个字段都有默认值, 因此只写想改的那几项。

| 项 | 说明 |
| --- | --- |
| `version` | 配置格式版本; 缺失按 0 处理, 然后逐级迁移 (`core::config::migrate`) |
| `paths.source` / `paths.archive` / `paths.keys` | 留空表示"自动决定", 界面上会显示最终落在哪里 |
| `archive.*` | 保留天数, 总量上限, 快照间隔, 去抖秒数, 归档范围 |

`wxmsg config` 把当前生效的配置与解析后的实际路径一起打印出来, `--init` 写默认配置文件。

### M7 完成情况

| 模块 | 职责 |
| --- | --- |
| `core::contact` | 读 `contact.db`, 把 wxid 换成人看得懂的名字 (备注 > 昵称 > 微信号 > wxid) |
| `core::media` | 按上面的规律定位媒体文件 |
| `core::export` | 把消息归一到统一结构, 再按 Markdown / JSON / CSV 写出 |
| `wxmsg export` | 命令行入口, `--with-revoked` 时把撤回记录按时间插回原处 |

三种格式共用同一套"什么算图片, 什么算文件"的判断, 因此不会互相跑偏。撤回的消息写成
`### 时间 发送者 类型 [撤回]`, 一眼能分辨。

实测 (真实数据):

| 检查 | 结果 |
| --- | --- |
| 单个会话 1346 条, CSV | 发送者全部解析成人名 |
| 8 个会话 30012 条, Markdown | 复制出 18 个文件附件 (mp3, zip, apk), 按名字与月份定位 |
| 一对一与群聊 | 发送者分别显示为联系人与群成员名 |

### 下一步

按依赖关系把顺序调整为: **M5 (CLI 与配置) -> M7 (导出) -> M6 (图形界面) -> M4 (密钥抓取)**。
M4 需要用户扫码, 放在最后实现, 届时再约验证时间。

M2 仍然欠一次现场验证 (发一条消息再撤回, 当场用 `recalls` 取回原文)。因为 `keep` 已经在跑
归档, 这一步什么时候做都不会再错过窗口了:

```shell
wxmsg snap    # 先取一次快照
wxmsg recalls --db "<数据根>/db_storage/message/message_1.db" --chat <该会话的内部 wxid>
```


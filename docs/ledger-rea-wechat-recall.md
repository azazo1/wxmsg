# 微信 4.1.7 撤回链路调查记录

工作目录: `/tmp/rea-wechat-recall/`
本文只记录**结论与证据**, 原始反汇编输出留在同目录的文件里, 不往对话里搬.

## 目标

| 项 | 值 |
| --- | --- |
| 应用 | `/Applications/WeChat.app`, 版本 4.1.7 (build 34371) |
| 主二进制 | `Contents/MacOS/WeChat`, 290 MB, universal |
| x86_64 切片 | 156,415,072 字节 |
| arm64 切片 | 147,375,040 字节, 文件内偏移 156,434,432 |
| 镜像基址 | 0x100000000 (`__TEXT` 段 vmaddr) |
| `__TEXT.__text` | 0x100015000 起, 92.29 MB, 文件偏移 86016 |
| 函数数 | **328,989** (来自 `LC_FUNCTION_STARTS`) |
| 符号 | 已剥离, 全文件非导入符号仅 2 个 |
| 指针修复 | `LC_DYLD_CHAINED_FIXUPS`, 文件内低 36 位为相对镜像基址的偏移 |
| ObjC | 元数据极小 (`__objc_classname` 2 KB, `__objc_methname` 40 KB), 基本为纯 C++ |

### 段区布局 (arm64 切片)

| 段区 | 地址 | 大小 | 文件偏移 |
| --- | --- | --- | --- |
| `__TEXT.__text` | 0x100015000 | 92.29 MB | 86016 |
| `__TEXT.__gcc_except_tab` | 0x105c7845c | 3.63 MB | 96961628 |
| `__TEXT.__const` | 0x1060185b0 | 33.45 MB | 100763056 |
| `__TEXT.__cstring` | 0x10818b88d | 0.96 MB | 135837837 |
| `__TEXT.__unwind_info` | 0x1082e33c8 | 1.34 MB | 137245640 |
| `__DATA_CONST.__const` | 0x1084403a0 | 3.74 MB | 138675104 |
| `__DATA.__data` | 0x108811140 | 2.06 MB | 142676288 |

## 方法

因为全量反编译在这台机器上不可行 (见"环境限制"), 采用"先机械定位, 再定点阅读"的方式.

工具都在工作目录, 都是**只读**解析, 不修改目标文件:

| 脚本 | 作用 |
| --- | --- |
| `macho.py` | 解析 fat/Mach-O 头, 段区表, `LC_FUNCTION_STARTS`, 镜像基址 |
| `refscan.py` | 扫描 `__text` 里的 ADRP+ADD/LDR, 找引用指定字符串的指令并归到函数 |
| `locate.py` | 定位字符串字面量所在段区与虚地址, 打印其所在可打印区段 |
| `callers.py` | 扫描直接跳转 (BL/B), 找调用指定地址的指令与所在函数 |
| `addrtaken.py` | 扫描 ADRP+ADD, 找"取某函数地址"的位置 (间接引用) |
| `slots.py` | 在所有段区找指向指定虚地址的指针槽, 覆盖 5 种编码 |
| `disas.py` | 定点反汇编 (objdump 流式过滤), 只取指定函数或地址范围 |
| `filter_functions.py` | 派生定范围的 Mach-O, 见"派生产物" |
| `dumptable.py` | 转储数据段指针槽并解链式修复编码 |

两个已知易错点, 后续脚本不要重犯:

1. `LC_FUNCTION_STARTS` 的增量是相对**镜像基址** (0x100000000) 的, 而 `__text`
   节区地址是 0x100015000, 两者差 0x15000. 混用会把函数地址算高一截.
2. ADRP 的编码是 `1 immlo[2] 10000 immhi[19] Rd[5]`, 高字节随 `immlo` 变化,
   必须用掩码 `(insn & 0x9F000000) == 0x90000000` 判断. 只比高字节 0x90 会漏掉
   四分之三的 ADRP, 我一开始就踩了这个坑.

## 已确认的观察

### 协议模板 (无引用)

`__TEXT.__cstring` 的 **0x1081bf959** 是一条 69 字节, 以 NUL 结尾的字符串:

```
<cgi reqid="0" respid="0" nettype="1" netstrategy="0">revokemsg</cgi>
```

**负面结论**: 代码里没有任何 ADRP+ADD 引用它; 全段区也没有任何指针槽指向它
(原值, 8 字节链式编码, 8 字节基址偏移, 4 字节槽位相对, 4 字节基址偏移五种编码
都试过, 合计 0). 另有第二处 `revokemsg` 出现在一条 23 KB 的 `<functionlist>` XML
里, 与撤回无关.**它不是链路的入口, 不要再从它入手.**

### 消息结构字段

两个函数是 `std::call_once` 的一次性初始化回调, 按名字登记字段:

| 回调 | 登记字段 | 传入者 | 守卫变量 |
| --- | --- | --- | --- |
| `0x1042162f0` (952 字节) | `session_name_id`, `history_id`, `server_id`, `is_revoke` | `0x10421620c` | 0x108c9d998 |
| `0x104295854` (1216 字节) | `to_user_name`, `svr_id`, `message_type`, `revoke_time`, `content`, `at_user_list` | `0x104295770` | 0x108ca3608 |

- 字段是**按名字查找**注册的 (每个字段先 `bl 0x1043314ec` 查名, 再 `bl 0x1043302b8`),
  所以别处是按序号访问, 不能靠字符串再找到访问点.
- 两个访问器各自还带一个 Meyers 单例: `operator new(0x110)` 分配 272 字节,
  调用 `0x1043fef58` 构造, 结果分别存到 0x108c9d988 和 0x108ca35f8.
- 推断 (非观察): 第二个字段集合含 `message_type` 与 `at_user_list`, 像**消息载荷**;
  第一个含 `is_revoke`, 像**消息记录/索引**. 也就是说撤回状态是消息记录自身的字段.
- `0x1043fef58` 被 115 个函数调用, 是公共的构造辅助.

### `revoke-history`

字符串 `revoke-history` 在 0x107471548, 被 3 个函数引用:

| 函数 | 被谁调用 |
| --- | --- |
| `0x104132c70` (556 字节) | `0x100573284`, 其调用者 `0x10057669c` |
| `0x10425edb8` (2000 字节) | `0x10425da78` |
| `0x1042656ec` (1388 字节) | `0x10425f588`, 而 `0x10425f588` 又被 `0x10425da78` 调用 |

`0x10425d848 → 0x10425da78 → {0x10425edb8, 0x10425f588 → 0x1042656ec}` 构成一个
"revoke-history" 子系统, 集中在 0x10425dxxx–0x104266xxx.

### 其他

- 字符串 `REVOKE-GROUP` 在 `__TEXT.__cstring` 的 0x1081db180, 前面一条是 `DATA`.
  代码无 ADRP+ADD 引用它, 但 `__DATA.__data` 的槽 **0x1088f8710** 指向它
  (链式编码, 原值 0x100000081db180). 同一张表里还有指向一条含 `revoke` 的
  `<functionlist>` XML 的槽 0x1088f7d60.**待查: 这张表是什么.**
- `__TEXT.__const` 里存在**定长 16 字节槽的名字表** (约 0x1072d11d0 起), 内容例如
  `CancelCheckRevok|AsyncCheckRevoke|HandleRevokeFail|Revoke list empt|message:%_, is s|MergeProcessResu|SendRevokeReques|extinfo nullptr`.
  按 NUL 结尾去读这块会得到拼接的假字符串, 长名被截断到 16 字节. 这些槽同样
  **没有任何代码引用**.
- 函数 `0x104826a28` 引用 `RevokeTask` 与 `group %_ revoke cdntask %_ reason %_:%_`,
  属于 **CDN 任务取消**, 与消息撤回无关, 已排除.

### 排除的框架

`ilink2`, `roam_server`, `roam_migration`, `andromeda` 里出现的 `revoke` 全部是
TLS 证书吊销 (`certificate revoked`, `X509_CRL_add0_revoked`, `sslv3 alert certificate revoked`)
和 CDN 任务取消 (`revoke cdntask`).**消息撤回只在主二进制内.**

## 派生产物

`wx-scoped` —— 140.5 MB 的 thin arm64 Mach-O, 由 `filter_functions.py` 从原文件派生:

- 只重写 `LC_FUNCTION_STARTS`, 函数项从 328,989 收缩到 **15,016**
  (范围 0x104130000-0x104400000 与 0x100570000-0x100580000)
- `__text` 字节, 段区表, 数据, 指针修复表**全部未改**, 虚地址与原文件一致
- 因此它与原文件不再字节一致, 属于派生产物, 分析时要走
  `--integrity-policy record-and-continue` 并如实说明来源

## 环境限制 (判断依据已更正)

- 本机 16 GB 内存, M1.
- 用 Ghidra 分析 `wx-scoped` (15,016 个函数, 4 GB 堆) 跑了约 15 分钟, 项目长到
  935 MB, 期间 8 核负载冲到 40.79. 已终止, 并核对了遗留 worker 的所有权
  (REA 的 `ownership.json` 记 pid 78903, 加 `pgrep` 比对命令行) 后杀掉.
- **更正 (用户指出)**: swap 的 `used` 是**累积分配量**, macOS 不会因为负载下降
  而把它归还, 所以这个数字**不能反映当前内存压力**. 先前把它当成"机器长期
  内存透支"的证据是错误推理, 由此限制堆大小也是错的. 负载高是 Ghidra 自己的
  并行分析线程, 不等于颠簸.
- **判断内存压力要用这些**:
  - `sysctl kern.memorystatus_vm_pressure_level` (1 = normal, 2 = warn, 4 = critical)
  - `memory_pressure` 的 `System-wide memory free percentage`
  - `vm_stat` 的 compressor 占用与 pageout 活动
  - 实测结果: pressure level = 1, free percentage 45%, compressor 5.87 GB
    —— **没有压力, 不需要为此调整参数**.
- 另一个真实的坑: REA 找 Ghidra 时会挑到 PATH 里最新的 JDK. 本机默认是 OpenJDK 26,
  需要显式 `export JAVA_HOME=/opt/homebrew/opt/openjdk@21`.
- 期间 Ghidra 曾被卸载过一次 (Cellar, opt, bin 三处都没了, `brew list` 里也没有),
  后由用户重装.

## Ghidra 分析尝试记录

| 次 | 保留函数 | 堆 | 关闭的分析器 | 结果 |
| --- | --- | --- | --- | --- |
| 1 | 15,016 | 4 GB | Aggressive Instruction Finder | 15 分钟未完成, 被我误判为内存颠簸而终止 |
| 2 | 586 | 2 GB | Aggressive Instruction Finder | 在 20 分钟超时前被我终止, 项目停在 919-935 MB |
| 3 | 586 | 3 GB | 再加 Decompiler Parameter ID | 同样停在 919 MB, 卡在同一阶段 |

第 3 次的 `ghidra.log` 在用户授权后读取, 给出关键信息 (工作区
`/var/folders/s9/qxfrtf9j44l1w_vpwcdl5nz00000gq/T/rea-ghidra-zB3J0x/ghidra.log`):

- 11:46:35 `ANALYZING all memory and code`
- 11:46:53 `CreateFunctionCmd Failed to create function at 104ca31a4 since its body
  contains referring thunk at 104ca3294`, 另一条同理在 10585cfd0. 这两个地址
  **远在保留范围之外**, 证明 Ghidra 会沿调用目标向整份二进制扩散. 因此收缩
  `LC_FUNCTION_STARTS` 只能减少**起始**函数, 不能限制总工作量.
- 11:47:16 `DecompilerSwitchAnalyzer hit non-returning function, restarting
  decompiler switch analyzer later`
- 11:47:44 `DecompileCallback Decompiling 1057d78a4: Unable to read bytes at ram:00000000`
- 之后约 7 分钟没有任何新行, 负载保持 5-6 —— 卡在 **Decompiler Switch Analysis**.

**教训**: 瓶颈是分析器本身, 不是函数表长度. 三个重分析器都要关:
`Aggressive Instruction Finder`, `Decompiler Parameter ID`, `Decompiler Switch Analysis`.

**保真度代价 (需要记住)**: 按 REA 自己的效率实验, 关掉 `Decompiler Switch Analysis`
会让部分被调函数的恢复代码体显著变短 (实验里一个被调函数从 604 字节降到 164 字节),
顶层计数相同**不能**证明调用图等价. 因此用这个配置得到的结果要标注该限制.

日志另外确认了导入细节, 与我的手工解析对照: Ghidra 用
`AARCH64:LE:64:AppleSilicon:default` 语言, 发现并修复了
**495,258 个 `DYLD_CHAINED_PTR_64_OFFSET` 指针**. 链式修复编码由 Ghidra 内置处理,
我的脚本是手工解的.

### 关键发现: 已发布版没有这个功能 (第 4 次仍卡住的真正原因)

第 4 次运行把 `Decompiler Switch Analysis` 也加入 `REA_GHIDRA_DISABLED_ANALYZERS`,
但日志仍出现 `DecompilerSwitchAnalyzer hit non-returning function`, 说明开关没生效.

查证结论: **已发布的 6.4.0 根本没有这个能力**.

```
grep -rl "ReaGhidraAnalysisOptions|disabledAnalyzers|DISABLED_ANALYZERS" \
  ~/.npm/_npx/af752b90495482d7/node_modules/rea-agents       -> 无匹配
ls 上述包的 bridge/ghidra/
  -> ReaGhidraApplySeeds / ReaGhidraBridge / ReaGhidraExtensions /
     ReaGhidraNoReturnFix / ReaGhidraPrepareCom     (没有 AnalysisOptions)
```

反向印证: 第一次运行的 JVM 命令行里 `-postScript` 只有 `ReaGhidraNoReturnFix.java`
和 `ReaGhidraBridge.java`, 没有 `ReaGhidraAnalysisOptions.java`.

因此那几次运行里 `REA_GHIDRA_DISABLED_ANALYZERS` **一直被静默忽略**, 每次跑的都是
完整默认分析器集. 这与 `--target-format raw-binary` 属同一类问题: **仓库 checkout
的文档描述的是尚未发布的能力**, `npx rea-agents@latest` 拿不到.

对策: 构建本地 checkout (`npm ci` + `npm run build:cached`) 取得该能力, 顺带也能
拿到 raw-binary. 已发布版的路由只保留 `--integrity-policy` 等已发布选项.

### 第 5 次: 真正的原因暴露 —— JVM 堆不足

用本地构建 (带 `ReaGhidraAnalysisOptions`) 关闭三个重分析器后重跑, 预脚本这次确实
执行了 (日志第 129 行列出了三个名字), 但 12:12:46 出现:

```
ERROR (ConstantPropagationAnalyzer) caught exception
  java.util.concurrent.ExecutionException: java.lang.OutOfMemoryError: Java heap space
    at ghidra.program.util.VarnodeContext.pushMemState
    at ghidra.program.util.SymbolicPropogator.applyPcode
    at ghidra.app.plugin.core.analysis.ConstantPropagationAnalyzer.analyzeLocation
ERROR (HeadlessAnalyzer) Abort due to Headless analyzer error: Java heap space
```

**结论: 前几次的"平台期"就是堆被逐步吃满的过程, 最终 OOM 中止.** 3 GB 堆对这个
可达集合不够.

这不是机器内存压力 (压力级别始终 1-2), 也不是分析器开关的问题 (开关这次已生效),
而是 `GHIDRA_HEADLESS_MAXMEM` 给小了.`-Xmx` 只是上限, 不会预留, 设大不浪费.

### 第 6 轮: 有界分析成功, 拿到伪代码, 找到 revokemsg 的真实构造点

**方法突破**: `bound_functions.py` 派生的有界文件 (1,895 个函数, `__text` 保留 0.47 MB)
分析只花 **147 秒**. 查询复用快照 `wx-bounded-snapshot.json`, 每次约 2 分 20 秒.

**沙箱副作用**: REA 在 `close_binary` 阶段要用进程表核对 Ghidra 清理情况, 沙箱拒绝
`ps` (`spawn EPERM`), 于是整次调用被报成 `cleanup_incomplete`, **掩盖真实结果**.
结果完整保存在 `details.partial_observation.raw_result.value` 里, 用 `reaq.py` 取出.

**消息载荷结构 (来自 `FUN_104295854` 的字段登记)**

| 字段 | 偏移 |
| --- | --- |
| `to_user_name` | 0x00 |
| `svr_id` | 0x18 |
| `message_type` | 0x20 |
| `revoke_time` | 0x24 |
| `content` | 0x28 |
| `at_user_list` | 0x40 |

`message_type` 与 `revoke_time` 是相邻的两个 32 位字段. 该回调同时**在运行时拼串**
(`'_','r','e','v','o','k','e','_'` + `"time"` = `_revoke_time`, 以及 `_svr_id_to_user_name`),
这就是静态字符串扫描找不到对应引用的原因.

**消息记录结构 (来自 `FUN_1042162f0` 的字段登记)**

| 字段 | 偏移 |
| --- | --- |
| `session_name_id` | 0x00 |
| `history_id` | 0x08 |
| `server_id` | 0x10 |
| `is_revoke` | 0x18 |

唯一索引名是 `_UNIQUEID` (同样运行期拼出: `0x49455551494e555f` = `_UNIQUEI` + `0x44` = `D`),
唯一键只用前三项, `is_revoke` 是记录上的标志位.**即: 每条消息一条记录, 撤回是记录上的标志.**

**`revokemsg` 的真实构造点: `FUN_102ce0c3c`**

伪代码要点:

- 运行期拼出字符串 `revokemsg`: `DAT_108bbc730` 依次写 `'r','e','v','o','k','e','m','e'`,
  再 `DAT_108bbc738 = 0x67617373` (`"ssag"`), `DAT_108bbc73c = 0x65` (`'e'`)
- 调 `FUN_104295770()` 取**消息载荷描述表**, 再 `FUN_1043ffadc(local_470, uVar3, 0x24)`
  读取 **偏移 0x24 = `revoke_time`**
- 用 `"revokemsg"` 与该值一起调 `FUN_1043f7740(...)`
- 另有 `local_474 = iVar2 - FUN_103e79194()` 的差值计算 (疑似时间差)

**这就是那条 `<cgi ...>revokemsg</cgi>` XML 模板在文件里看似无引用的原因: 名字是拼出来的.**

**同簇的 `FUN_102cfb2b4` 与 `FUN_1022df7f4` 结构相同** —— 都是消息对象的销毁/清理:
遍历链表调虚析构 `operator_delete(ptr, 0x158)`, 并用描述表访问器 (`FUN_104295770` /
`FUN_10421620c`) 取 `+0x28` 子对象, 读 `+0x80` 的字节标志. 两者分别对应消息载荷视图与
消息记录视图.

## 当前路线

按用户决定, **不用 Ghidra**, 用 objdump 定点反汇编人工追链. 优先级:

1. `__DATA.__data` 里指向 `REVOKE-GROUP` 的那张表 (待查, 可能是分发表).
2. 使用消息载荷描述表的 5 个函数 (0x102cdb6a8, 0x102cdfddc, 0x102ce0804,
   0x102ce0c3c, 0x102cfb2b4) —— 走 `revoke_time` 的路径.
3. 使用消息记录描述表的 7 个函数 (0x1022d94d4, 0x1022d9bac, 0x1022da02c,
   0x1022dace8, 0x1022db14c, 0x1022ddff8, 0x1022df7f4) —— 走 `is_revoke` 的路径.

## 未解问题

- 撤回通知从哪一层进入 (同步层? 推送层?), 入口函数是谁.
- `is_revoke` 与 `revoke_time` 分别在哪被写入, 在哪被读取.
- 撤回后本地消息内容是被删除还是只改标记.
- 界面更新是通过什么回调触发的.

## 更新记录

### 第 2 轮: 排除 CDN 状态表, 发现界面文案, 确认方法论限制

**CDN 状态名表 (排除)**

`__DATA.__data` 的 0x1088f8650 起是一张**纯字符串指针表**, 内容是 CDN / 媒体下载
任务的状态与失败原因名, 按出现顺序节选:

```
UNDER-MOBILE, FAILED, TIMEOUT, PARTIAL-WORKED, N/A, CDN-STREAM, CDN-HTTP, FTN,
STD-HTTP, WAIT, RUNNING, CANCELED, COMPLETED, NEW, CACHE, CACHE, SERVER, REQDATA,
CLOSE-PCDN, OPEN-PCDN, QUIT, CREATE-CDN, CLOSE-UGC, GRANT-GROUP, REVOKE-GROUP,
BUFFERING-BEGIN, BUFFERING-END, RESUME, PAUSE, Storage, Streaming, Preload,
QueryInfo, NO-CONFIG, ACCEPTED, NETWORK-MISMATCH, TIMESTAMP-EXPIRED,
INVALID-VIDEOFLAG, FORCE-UGC, INSUFFICIENT-DATA, NO-URL, NO-RESUME-DATA,
UNKNOWN, NO-NETWORK, TOO-MANY-RETRY, CONNECT, READWRITE
```

槽 0x1088f8710 指向 `REVOKE-GROUP`. 这是"批量取消 CDN 任务", 与消息撤回无关,
和前面 0x104826a28 里的 `group %_ revoke cdntask %_ reason %_:%_` 相互印证.
**排除, 不要再追.**

**方法论限制 (重要)**

字段名只被**序列化器的描述表构造器**引用. 业务逻辑用字段偏移而不是名字, 所以
**靠字符串引用到不了撤回的业务逻辑**. 中文界面文案虽然在 `__DATA.__data` 里找到,
但 ADRP+ADD 扫描同样 0 命中, 说明引用经过了数据段指针表, 需要**两跳**
(代码 -> 指针槽 -> 字符串).

**界面文案 (`__DATA.__data`)**

| 虚地址 | 文本 |
| --- | --- |
| 0x1088c008a | 撤回群公告 |
| 0x1088c2623 | 撤回将同时清空群公告 |
| 0x1088c2dcd | 撤回 |
| 0x1088c3c21 | 已撤回 |
| 0x1088c59e7 | 消息被撤回，文件已无法下载 |
| 0x1088c6d50 | 部分消息被撤回，无法转发 |
| 0x1088ce33f | 你撤回了一条消息 |
| 0x1088cdbd3 | 不能重新编辑超过5分钟的消息 |
| 0x1088d0a5e, 0x1088d1960 | 重新编辑 |

未找到 `对方撤回`, 说明对方撤回的提示语可能由模板加名字拼出.

**下一手**: 对界面文案虚地址做指针槽扫描 (`slots.py`), 拿到槽地址后再用
`addrtaken.py` 扫代码, 完成两跳定位.

### 工具修正记录

- `addrtaken.py` 只处理 ADRP+ADD/LDR 相对寻址, 不处理 LDR literal (0x58xxxxxx),
  也不处理经数据段指针表的间接引用. 定位不到引用时先怀疑这个, 再做两跳扫描.
- 已用对照实验验证过扫描器本身: 拿反汇编里亲眼见过的 `adrp x9, 0x108c9d000` +
  `ldr x9, [x9, #0x9a8]` 去扫, 精确命中函数 0x10421620c 的 0x104216210. 所以
  "0 命中" 是真实的, 不是工具坏了.**每次得到 0 命中都要先做这样一个对照.**

### 第 3 轮: 界面文案池无引用, 读了 is_revoke 簇的最小函数

**界面文案池 (`__DATA.__data` 0x1088c0000-0x1088d3000, 约 72 KB)**

- 代码 ADRP+ADD 取址: **0 命中**
- 数据段指针槽 (链式编码 / 基址偏移 / 原值): **0 命中**

两个方向都没有任何引用, 说明它不是逐条寻址的字符串, 而是被某个通用加载器整体
消费的数据块 (资源块).**界面文案这条线到此为止, 不要再做单条串的引用扫描.**

**函数 0x1022df7f4 (436 字节, is_revoke 簇)**

读出的行为: 这是一个**消息记录对象的销毁/清理**函数.

- 取 `this` 到 x19, 先 `ldr x0, [x19+8]` 调 0x1043fa084 做前置判断, 成立则直接返回 1
- 遍历链表销毁节点: `ldr x8, [x0, #0x10]!; ldr x8, [x8]; blr x8` 调虚析构,
  再 `operator delete(ptr, 0x158)` —— 每个节点 **344 字节**, 虚表指针在 +0x10
- 在处理过程中调用 **0x10421620c** (`is_revoke` 描述表访问器), 然后
  `add x0, x0, #0x28` 取子对象, 再读 `[x0, #0x80]` 的一个字节标志决定要不要调
  0x1043d2a38 / 0x1043dc608
- 栈上放过 `0x1087482f0+0x10` 作为临时对象的虚表指针

得出的结构事实: 描述表对象 272 字节, `+0x28` 是子对象基址, `+0x80` 是一个字节标志.

**结论**: `is_revoke` 描述表的消费者是消息记录的**生命周期与字段访问**代码
(0x1022dxxxx 簇), 不是撤回的判定逻辑. 这与"字段名只被序列化器引用"的结论一致.

## 方法论结论 (截至第 3 轮)
这个二进制**高度数据驱动**: 协议模板无引用, 字段名只被描述表构造器引用, 界面
文案在无引用的资源块里, 函数地址经 `std::call_once` 与间接表注册. 因此
**字符串与地址扫描到不了撤回的判定逻辑**, 只能到达描述表, 序列化器, 生命周期代码.

在只用 objdump 的前提下继续, 就必须逐个读 C++ 函数的汇编并手工推导数据流.
实测节奏: 约 5 到 10 次工具调用推进一层, 而且每层都可能分支. 届时尚需确认
`is_revoke` / `revoke_time` 的写入点与读取点, 再往上追到通知与界面.

### 第 7 轮: 撤回的两条命令名与分发点

命令名都是运行期拼出来的, 这也是静态扫描找不到它们的原因:

- `revokemsg`: `DAT_108bbc730` 依次写 `'r','e','v','o','k','e','m','e'`, 再
  `DAT_108bbc738 = 0x67617373` (`"ssag"`), `DAT_108bbc73c = 0x65` (`'e'`)
- `revokebatchmessage`: `DAT_108bbc740 = 'r'`, 接着 `"evokeba"`, 0x108bbc748 起
  `"tchmessa"`, `DAT_108bbc750 = 0x6567` (`"ge"`)

消息载荷描述表的 4 个使用点, 都先 `FUN_104295770()` 取表, 再用
`FUN_1043ffadc(obj, 表, 偏移)` 取字段:

| 函数 | 取的字段 | 命令名 | 之后的去向 |
| --- | --- | --- | --- |
| `0x102cdb6a8` | - | `revokebatchmessage` | `FUN_1043f7890(对象, 表)` |
| `0x102cdfddc` | 0x18 `svr_id`, 0x00 `to_user_name` | `revokemsg` | `FUN_102ce02e4` |
| `0x102ce0804` | 0x18 `svr_id`, 0x00 `to_user_name` | `revokemsg` | 待读 |
| `0x102ce0c3c` | 0x24 `revoke_time` | `revokemsg` | `FUN_1043f7740` -> `FUN_10220afcc` |

两个中间层的行为:

- `FUN_1043f7740(p1, 名字, p3..p6)`: 先 `FUN_1043f7660` 构造临时对象, 用
  `FUN_1043fae04(构造器, 名字)` 按名字查, 再用 `FUN_1043fa8f0` / `FUN_1043c56a4` 包装,
  最后调 `FUN_10220afcc(p1, 查到的东西, p3..p6)`. 即**按名字分发的派发器**.
- `FUN_1043ffadc(obj, param_2, 偏移)`: `FUN_1043ff468(param_2, 偏移)` 取字段描述,
  `FUN_1043c54e0(obj)` 取子对象, 把值写进 `子对象+0x48`, 原始值写 `子对象+0x80`,
  派生值写 `obj+0xf0`. 即**按偏移取字段**.

**范围扩展**: `FUN_10220afcc` 在 0x10220xxxx, 原先不在保留范围内. 已重新派生
`wx-bounded2` (3,611 个函数, 0.76 MB 代码, 快照 `wx-bounded2-snapshot.json`,
分析耗时 142 秒).

### 第 8 轮: 存储形态与加密方案 (重心转到"内容能否恢复")

**本机存储位置**

`~/Library/Containers/com.tencent.xinWeChat/Data/Documents/xwechat_files/<wxid>/`

| 目录 | 内容 |
| --- | --- |
| `db_storage/message/` | 加密库: `message_0.db` 180.8 MB, `message_1.db` 21.5 MB, `biz_message_0.db` 79.0 MB, `media_0/1.db` |
| `db_storage/MMKV/` | `*.mmkv` 与 `.crc`, 以及 `storage_common` |
| `msg/attach`, `msg/file`, `msg/video` | 附件与媒体, **普通文件** |
| `cache/<年-月>/Message/<md5>` | 按哈希命名的缓存块 |

**加密方案 (观察)**

- 四个库的前 16 字节都不是 `SQLite format 3\0`, 是随机字节 → **加密存储**
- `kvdb` 与 `MMKV` 文件同样不是明文 → 那些存储也被加密
- 二进制里 `sqlcipher` 2 次, `cipher_page_size` 4 次, `kdf_iter` 4 次
  → **SQLCipher, 页大小与 KDF 迭代可配**
- `PRAGMA key` 与 `sqlite3_key` 字符串 **0 命中** → 密钥不是用 PRAGMA 字符串设置,
  而是直接调静态链接的 C API
- `PBKDF2` 8 次, `HKDF` 4 次, `dbkey` 3 次, `db_key` 2 次, `device_id`/`deviceid` 存在
  → 存在本地推导机制
- 每个库旁有三件套 `-first.material` / `-incremental.material` / `-last.material`,
  `message_1.db-incremental.material` 当天仍在更新
- `.material` 19 次, `kvdb` 4 次, `db_storage` 6 次 (都在 `__TEXT.__const`, 不在 `__cstring`)

**已定位的加密与存储相关代码**

| 函数 | 引用的字符串 | 说明 |
| --- | --- | --- |
| `0x1043d2f34` | `cipher_page_size` | 薄包装, 拼名字后调 `0x1043d2e00` |
| `0x1043d2e14`, `0x1043d2ea4` | `cipher...` | 同类薄包装 |
| `0x104b37ab0` | `cipher...` | 在范围外 |
| `0x1034eaa64` | 0x10739bdf1 起连续 8 个名字 | 像字段或表名列表的消费者 |
| `0x103d433d8`, `0x103d43e38` | `db_storage` 区 | 疑为路径构造或开库 |
| `0x103e5f4f4` | 0x1074073d8 区多个名字 | 引擎配置 |
| `0x104016ed8` | `kvdb` 区 | KV 存储 |

**对"恢复"的直接结论**

- 文本消息在加密库里 → 不解密读不到
- 附件与媒体是普通文件: `msg/video` 2628 个 / 3.8 G, `msg/file` 441 个 / 601 M,
  `msg/attach` 18 个 / 3.2 G → **这部分不依赖数据库密钥**
- `*.material` 是增量材料, 可能保留旧版本 → 值得作为恢复方向
- **Time Machine 未配置**, 本地快照只有系统更新用的 → 无备份可回退

**仍未拿到**: 密钥的实际来源 (本地推导的具体输入, 或所在文件, 或服务端下发).
需要定位"打开库并把密钥交给 SQLCipher"的那个点.

**方法记录**: 存储层名字都在 `__TEXT.__const` (不在 `__cstring`), 因此定位时不能只按
NUL 结尾取串. 可行做法是先按字节找到出现位置, 换算成虚地址, 再把该地址附近的
窗口交给范围版取址扫描 (`addrtaken.py` 支持 `起址-止址` 形式).

### 第 9 轮: 撤回实验 (动态观察, 不用调试器)

实验设计: 多个时点对存储取快照, `db_storage` 逐文件 sha256, `msg` 下逐文件登记大小与
时间, 然后两两对比. 工具 `snapshot_store.py`. 时点: baseline, sent, out, revoked.

**会话附件目录名 = 会话用户名的 md5**

- 对照: `md5("filehelper")` = `9e20f478899dc29eb19741386f9343c8`, 该目录确实存在于
  `msg/attach`, 假设成立
- 因此只要知道确切用户名, 就能反查该会话在磁盘上的痕迹
- 所以别名换不来目录名: 拿别名直接算 md5 对不上, 必须先用 `contact.db` 换成 username

**消息落盘形态**

- 收发文件是**明文普通文件**, 保留原名: 发出的 `demo.txt` 落在
  `msg/file/2026-10/demo.txt` (5 字节)
- 收到的图片同时产生 `msg/file/<年-月>/image-<时间戳>.gif` (10 MB)
  与 `msg/attach/<会话md5>/<年-月>/Img/<哈希>.dat` (缩略图, 40 KB)

**撤回的行为 (决定性观察)**

| 时点 | `msg` 下的变化 |
| --- | --- |
| baseline -> sent | 新增 `msg/file/2026-10/image-...gif` 与 attach 下的 `.dat` |
| sent -> out | 新增 `msg/file/2026-10/demo.txt` |
| out -> revoked | **消失 `msg/file/2026-10/demo.txt`** |

撤回后按名字在容器内搜索, 没有任何 `demo.txt` 副本, 也没有回收站类目录.
→ **撤回直接删除明文文件副本**, 不是移动, 也不是只打标记.

**撤回改写的数据库 (out -> revoked)**

- `message/message_1.db` 及其 `-wal`, `-shm`, `-incremental.material`
- `message/message_fts.db` (全文索引) 及其 `-wal`, `-shm`
- `message/message_resource.db` 及其 `-wal`, `-shm`
- `session/session.db` 及其 `-wal`, `-shm`, `-incremental.material`

所有文件**大小不变** → 页内改写, 没有回收页. `session.db` 被改写与"聊天列表要刷新出
撤回提示"一致, 可作为界面更新的存储侧证据.

**加密覆盖面**

`db_storage` 下所有库的前 15 字节都不是 `SQLite format 3`: `message_0/1.db`,
`message_fts.db`, `message_resource.db`, `contact.db`, `contact_fts.db`,
`session.db`, `bizchat.db`, `emoticon.db`, `favorite.db`, `favorite_fts.db`,
`general.db`, `hardlink.db`, `head_image.db`, `media_0/1.db`, `biz_message_0.db`,
以及各 `.kvdb`. → **整个数据库层加密, 全文索引也不例外**.

**本机无备份**: Time Machine 未配置; 本地快照只有系统更新用的; `xwechat_files/Backup`
只有 44 KB 的迁移壳 (`backup.attr`, `alt_name.dat`, `roam_device_info.dat`).

**对恢复问题的小结**

- 附件与文件: 撤回会**主动删除**明文副本, 恢复只能指望文件系统层面的残留块
- 文本: 全库加密, 磁盘上没有可读副本; 撤回是否清除正文必须先拿到密钥
- 图片: 缩略图 `.dat` 与 `cache/<年-月>/Message/<md5>` 是否随撤回一起删除, 尚未验证

### 第 10 轮: 开源实现调研与取密钥的可行路线

**本机加密参数 (来自开源项目的实测, 与我们的静态观察一致)**

| 参数 | 值 |
| --- | --- |
| 算法 | AES-256-CBC |
| 主密钥 | 原始 32 字节, 直接使用, **不做 PBKDF2** |
| HMAC | HMAC-SHA512, mac key = PBKDF2-HMAC-SHA512(key, salt ^ 0x3a, 2) |
| 页大小 | 4096 |
| 保留区 | 80 (IV 16 + HMAC 64) |
| 盐 | 文件前 16 字节 |
| 每个库 | 独立的密钥与盐 |

参数已知意味着: **只要拿到密钥, 解密可以自己用 Python 实现, 不需要装 SQLCipher**;
而且首页 HMAC 校验可以用来验证候选密钥。

**为什么内存扫描在 4.1.x 取不到密钥**

- 密钥不再以 `x'<64 hex key><32 hex salt>'` 字符串形式驻留内存
- `mach_vm_read` 读不到持有密钥的内存区域 (lldb 的 debugserver 可以, 因为它看到
  8.7 GB 而 Mach VM 只看到 2.7 GB)

**取密钥的方法 (与版本无关)**

在 CommonCrypto 的 `CCCryptorCreate` 上断点或挂钩. arm64 上第 4 个参数 (key) 在 x3,
第 5 个 (keyLength) 在 x4; 命中且 `x4 == 32` 时读取 x3 处的 32 字节, 即某库的原始密钥.
密钥只在微信真正解密该库时出现, 所以抓取时必须手动打开会话并滚动历史.

**本机适用性验证 (结论: 成立)**

主程序动态导入 `_CCCryptorCreate`, `_CCCryptorFinal`, `_CCCryptorRelease`,
`_CCCryptorUpdate`, `_CCKeyDerivationPBKDF` → 走动态链接, 可被 interpose.

**开源实现 (2026-10 时点)**

| 仓库 | 方式 | 要求与验证环境 |
| --- | --- | --- |
| `rmqg/wechat-mac-export` | lldb 断点 CommonCrypto | 关 SIP, 重签名, sudo; 验证于 4.1.11 |
| `TimFang4162/wechat4-macos-frida-export` | 复制 App 并注入 Frida Gadget | **SIP 可保持开启**, 不改官方 App, 需完全磁盘访问与一次独立扫码; 验证于 4.1.13 |
| `tzwkb/wechat-decrypt` | Agent Skill, macOS 与 Windows | - |
| `ydotdog/wechat-export-macos` | 解密导出 | 上面 Frida 项目的解密器基础 |
| `Thearas/wechat-db-decrypt-macos` | 已清空, README 只剩一句免责声明 | 不可用 |

**顺带确认的事实**

- lldb 无法附加的直接原因: Xcode 的 lldb 解析微信主程序 Mach-O 符号表时导出 trie
  递归过深, `ObjectFileMachO::ParseSymtab` -> `ParseTrieEntries` 栈溢出崩溃
- 消息表名 = `Msg_<md5(用户名)>`, 与我们在 `msg/attach` 下发现的一致
- `message_content` 格式为 `"<发送者wxid>:\n<载荷>"`
- `local_type` 高位打包子类型, 真实类型 = `local_type & 0xFFFFFFFF`
- `WCDB_CT_*` 列标记的行内容以 zstd 压缩 (帧头 `28 b5 2f fd`)
- **撤回在库里表现为一条系统消息**, 导出工具会把"撤回提醒"渲染为文本

**本机工具链**: uv, clang, python3, git, lldb, Xcode 均已就绪; 可用空间 141 GB;
frida 未安装 (Gadget 方案自带, 不需要系统安装 frida).

### 第 11 轮: 拿到密钥并解密, 撤回的本地行为与可恢复性 (决定性结果)

**取密钥 (开源路线成功)**

用 `TimFang4162/wechat4-macos-frida-export`: 复制微信副本, 注入 Frida Gadget,
拦截 CommonCrypto 的 `CCCryptorCreate` 等入口抓取每个库的 raw key.

- 抓到 24 个候选密钥, 对 **19/19** 个库完成 HMAC 校验映射, **全部解密成功** (0.4 GB)
- 针对本机环境的两处修复:
  - 该项目漏了移除 `com.apple.developer.team-identifier`. ad-hoc 签名的副本却声明
    腾讯 Team ID, 会被 AMFI 拒绝, 表现为 `Launchd job spawn failed` (errno 153).
    补上后副本即可启动.
  - `cp -cR` 的 clonefile 对带数据保险箱保护的缓存文件被拒. 改为普通复制, 并只复制
    `db_storage` 等必要目录 (跳过 `msg` / `cache` / `business`, 省约 8 GB).

**会话表定位**

- 表名 = `Msg_<md5(username)>`, username 是 **wxid**, 不是微信号别名
- `alias_a` 是别名, 从 `contact.db` 反查到 username = `wxid_a1b2c3d4e5f60000`,
  表名 = `Msg_<md5(username)>`
- 同一会话分片在 `message_0.db` (历史) 与 `message_1.db` (近期)

**撤回的本地行为 (决定性观察)**

撤回后该会话表的 local_id 序列:

| local_id | 时间 | 类型 | 内容 |
| --- | --- | --- | --- |
| 2154 | 10-13 14:30:16 | 1 | `<示例消息>` |
| 2155 | 10-13 14:31:32 | 10000 | `<sysmsg type="revokemsg">...你撤回了一条消息...` |
| 2156 | 10-13 14:31:35 | 1 | `<另一条示例消息>` |

- **被撤回的 `demo.txt` 文件消息行已不存在**, 而 2155 本该是它
- SQLite 非 AUTOINCREMENT 的 local_id 取 `max+1`, 删除最大行号后会被下一次插入复用,
  观察到的编号复用与此吻合
- 结论: **撤回是删除本地消息行, 随后插入一条 type=10000 的 `revokemsg` 系统消息**

**内容可恢复 (决定性证据)**

被删行的页面镜像仍留在 `message_1.db-wal` 的**被取代帧**里 (页号 5063, 帧 17 与 18).
用同库密钥按 SQLCipher 页格式解出后, 其中的 zstd 流可完整解压:

```xml
<msg><appmsg><title>demo.txt</title><type>6</type>
<appattach><totallen>5</totallen>
<attachid>@cdn_..._<hash>_1</attachid>
<cdnattachurl>305f0201...</cdnattachurl></appattach></appmsg></msg>
```

即**文件名, 大小 (5 字节), CDN 引用全部还原**. WAL 共 1018 帧, 被取代帧仍保留
删除前的页面版本.

### 第 12 轮: 拦截撤回与抗更新能力分析

**拦截必须挂在哪**

撤回的两个动作是"删除消息行"与"插入 revokemsg 系统消息", 所以拦截必须发生在删除之前.

| 钩子位置 | 抗更新 | 能保住什么 |
| --- | --- | --- |
| `unlink` / `rename` (libSystem ABI) | 最稳 | 撤回会删 `msg/file/...` 明文附件, 挂这里可保住附件 |
| CommonCrypto | 稳, 已验证 4.1.7 | 只能取密钥, 不能阻止删除 |
| 微信内部删除逻辑 | 脆 | 依赖地址或内部结构, 更新即可能失效 |

**更稳的替代: 持续归档**

既然删除前的页面留在 WAL 里, 只要在 WAL 被覆盖前归档加密库与其解密产物,
就**不需要拦截**. 该路线只读数据, 不修改 app, 不注入, 风险显著更低.
代价是每次微信更新后密钥可能变化, 需要重跑密钥抓取 (流程已跑通).

**抗更新的分层结论**

- 稳: 复制 + 注入 Gadget + 重签名这套构建步骤; CommonCrypto 的 ABI; 文件级钩子
- 会变: per-DB 密钥 (更新后重抓); SQLCipher 参数 (当前 AES-256-CBC / 4096 /
  reserve 80); 表结构 (`Msg_<md5(wxid)>` 只是当前规则); 撤回的落点
- 史实: 4.1.10 那次更新让既有工具集体失效 (密钥不再以字符串驻留内存, SQLCipher
  符号被剥离). 因此"永久可用"不成立, 只能做到"更新后可较快重新适配",
  这正是有界分析流程的用处.

**其他约束**: 重签名并注入的副本不宜作为日常微信长期使用, 存在签名校验与账号
受限的风险. 若要长期稳定, 应关闭微信自动更新, 并保持一个已验证版本.

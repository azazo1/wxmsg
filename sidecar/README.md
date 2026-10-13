# sidecar

密钥抓取用的 Frida 钩子脚本。

## hook.js

在目标进程内钩住 CommonCrypto 的四个入口, 把经过的 32 字节密钥上报:

| 函数 | 参数约定 (arm64) |
| --- | --- |
| `CCCrypt(op, alg, options, key, keyLength, iv, ...)` | 密钥在 `args[3]`, 长度在 `args[4]` |
| `CCCryptorCreate(op, alg, options, key, keyLength, iv, ...)` | 密钥在 `args[3]`, 长度在 `args[4]` |
| `CCCryptorCreateWithMode(op, mode, alg, pad, iv, key, keyLength, ...)` | 密钥在 `args[5]`, 长度在 `args[6]` |
| `CCKeyDerivationPBKDF(alg, password, pwLen, salt, saltLen, prf, rounds, derivedKey, derivedKeyLen)` | 派生结果, 用于识别 SQLCipher 的 mac key 派生 |

只上报长度等于 32 的密钥 (SQLCipher 的原始密钥长度)。上报格式为 `KEY32 <函数名> <hex>`,
控制端按前缀过滤即可识别。

### 断网开关

脚本顶部有 `DENY_REMOTE` 常量。置为 `true` 时会**替换** `connect` 与 `connectx` 本体,
拒绝对外连接, 只放行回环 (Gadget 需要 `127.0.0.1`)。用于在不动机器网络的前提下,
判断某个行为是否依赖服务端。

用途已经验证过: 置为 `true` 后微信连登录二维码都取不到, 说明登录全程依赖服务端。

两点实现经验:

- 必须用 `Interceptor.replace` 加 `NativeCallback`, 并在失败时写 `__error()`。
  用 `Interceptor.attach` 只能伪造返回值, 内核层的连接照样建立
- 上报的消息前缀若不在控制端的白名单里会被静默丢弃, 排查时容易误判为"钩子没生效"

## 控制端

抓取由 `wxmsg capture` 驱动: 它负责复制副本, 重定向家目录, 调起 `capture.py`, 最后用 HMAC 把
候选密钥配到各个库。单独手工运行也可以:

```shell
# 副本启动后 (Gadget 监听 127.0.0.1:27042, on_load 为 wait)
frida -H 127.0.0.1:27042 -l sidecar/hook.js
```

或让 `capture.py` 自己启动副本并收集:

```shell
uv run --project sidecar python sidecar/capture.py \
  --target /path/to/WeChat.app/Contents/MacOS/WeChat \
  --hook sidecar/hook.js --home /path/to/副本家目录 \
  --out keys.json --timeout 900
```

依赖由 `sidecar/pyproject.toml` 声明, `uv run` 会把它装进 `sidecar/.venv`, 不需要手工安装。

### 权限

macOS 不允许进程随便挂到别的进程上。若报 `PermissionDeniedError: unable to access process`,
说明当前终端没有调试权限: 换一个被授权的终端, 或在 系统设置 -> 隐私与安全性 -> 开发者工具
里把终端加进去。受限沙箱里也会出现同样的报错。

### 候选密钥不是最终答案

钩子只能给出"经过 CommonCrypto 的 32 字节值", 里面混着大量与微信库无关的密钥。哪把开哪个库,
由主程序读库文件、验页 1 的 HMAC 来判断, 因此抓取与匹配是分开的两件事。

## 出处

来源于 `TimFang4162/wechat4-macos-frida-export` (WTFPL) 的 `hook.js`,
本项目在其基础上增加了断网开关, 版本标记与 `connectx` 处理。原项目验证于微信 4.1.13,
本机在 4.1.7 上实测可用。

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

```shell
# 副本启动后 (Gadget 监听 127.0.0.1:27042, on_load 为 wait)
frida -H 127.0.0.1:27042 -l sidecar/hook.js
```

或使用 frida-python 自行连接, 把 `KEY32` 上报逐行落盘。

## 出处

来源于 `TimFang4162/wechat4-macos-frida-export` (WTFPL) 的 `hook.js`,
本项目在其基础上增加了断网开关, 版本标记与 `connectx` 处理。原项目验证于微信 4.1.13,
本机在 4.1.7 上实测可用。

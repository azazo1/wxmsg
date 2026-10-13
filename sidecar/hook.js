// Hook CommonCrypto AES/KDF entry points in WeChat; report keys.
// 适用于微信 4.1.10+（进程内存中不再缓存 x'<hexkey>' 字符串的版本）：
// 派生后的 per-DB raw key 只在加解密瞬间经过 CommonCrypto。
//   CCCrypt(op,alg,options,key,keyLength,iv,...)              key=args[3] len=args[4]
//   CCCryptorCreate(op,alg,options,key,keyLength,iv,...)      key=args[3] len=args[4]
//   CCCryptorCreateWithMode(op,mode,alg,pad,iv,key,keyLen,..) key=args[5] len=args[6]
//   CCKeyDerivationPBKDF(alg,password,pwLen,salt,saltLen,prf,rounds,derivedKey,derivedKeyLen)
//     — SQLCipher4 mac key 派生：password=raw enc_key(32B), rounds=2, prf=SHA512
'use strict';

// 断网模式: 为 1 时拒绝该进程的一切对外连接, 只放行回环 (Gadget 需要 127.0.0.1)。
// 目的: 判定库密钥的解锁是否依赖服务端, 而不影响机器本身的网络。
const DENY_REMOTE = false;
send('HOOKVERSION 2 netblock=' + DENY_REMOTE);

const seen = new Set();

function toHex(buf) {
  return Array.from(new Uint8Array(buf), b => ('0' + b.toString(16)).slice(-2)).join('');
}

function logKey(tag, key, len) {
  try {
    if (len !== 32) return;
    if (key.isNull()) return;
    const hex = toHex(key.readByteArray(32));
    if (seen.has(hex)) return;
    seen.add(hex);
    send('KEY32 ' + tag + ' ' + hex);
  } catch (e) { /* unreadable pointer — skip */ }
}

function resolve(name) {
  try { if (Module.findGlobalExportByName) { const a = Module.findGlobalExportByName(name); if (a) return a; } } catch (e) {}
  try { if (Module.getGlobalExportByName) return Module.getGlobalExportByName(name); } catch (e) {}
  try { return Module.getExportByName(null, name); } catch (e) {}
  return null;
}

let hooked = 0;
for (const name of ['CCCrypt', 'CCCryptorCreate', 'CCCryptorCreateWithMode']) {
  const addr = resolve(name);
  if (addr === null) { send('MISS ' + name); continue; }
  const keyIdx = name === 'CCCryptorCreateWithMode' ? 5 : 3;
  const lenIdx = keyIdx + 1;
  Interceptor.attach(addr, {
    onEnter(args) { logKey(name, args[keyIdx], args[lenIdx].toInt32()); }
  });
  hooked++;
}

const pbkdf = resolve('CCKeyDerivationPBKDF');
if (pbkdf !== null) {
  Interceptor.attach(pbkdf, {
    onEnter(args) {
      try {
        const pwLen = args[2].toInt32();
        const rounds = args[6].toInt32();
        // SQLCipher 4 的 HMAC key 派生以 32-byte raw enc key 为 password，
        // rounds=2。只读取该输入，不读取 ABI 相关栈参数或输出缓冲区。
        if (pwLen === 32 && rounds === 2) {
          logKey('CCKeyDerivationPBKDF', args[1], pwLen);
        }
      } catch (e) {}
    }
  });
  hooked++;
} else {
  send('MISS CCKeyDerivationPBKDF');
}

if (DENY_REMOTE) {
  const errnoFn = resolve('__error');
  let denied = 0;

  function remoteOrNot(sa) {
    // 返回 true 表示这是需要拒绝的对外地址; false 表示放行 (回环或不可读)
    try {
      if (sa.isNull()) { return false; }
      const family = sa.add(1).readU8();
      if (family === 2) {
        const port = (sa.add(2).readU8() << 8) | sa.add(3).readU8();
        const ip = [4, 5, 6, 7].map(function (i) { return sa.add(i).readU8(); }).join('.');
        if (ip.indexOf('127.') === 0) { return false; }
        send('DENY4 ' + ip + ':' + port);
        return true;
      }
      if (family === 30) {
        const raw = new Uint8Array(sa.add(8).readByteArray(16));
        let loopback = raw[15] === 1;
        for (let i = 0; i < 15; i += 1) { if (raw[i] !== 0) { loopback = false; } }
        if (loopback) { return false; }
        send('DENY6');
        return true;
      }
    } catch (e) { /* 结构不可读则放行 */ }
    return false;
  }

  function failErrno() {
    if (errnoFn !== null) {
      try { errnoFn().writeInt(51); } catch (e) { /* ENETUNREACH */ }
    }
    return -1;
  }

  // 必须用 replace 而不是 attach: attach 只能伪造返回值, 内核层连接已经建立。
  const connectFn = resolve('connect');
  if (connectFn !== null) {
    const origConnect = new NativeFunction(connectFn, 'int', ['int', 'pointer', 'uint']);
    Interceptor.replace(connectFn, new NativeCallback(function (fd, sa, len) {
      if (remoteOrNot(sa)) { denied += 1; return failErrno(); }
      return origConnect(fd, sa, len);
    }, 'int', ['int', 'pointer', 'uint']));
  }
  // Network.framework 走 connectx, 目标地址在 sa_endpoints_t 的 +24 处
  const connectxFn = resolve('connectx');
  if (connectxFn !== null) {
    const origConnectx = new NativeFunction(connectxFn, 'int',
      ['int', 'pointer', 'uint', 'uint', 'pointer', 'uint', 'pointer', 'pointer']);
    Interceptor.replace(connectxFn, new NativeCallback(
      function (fd, endpoints, associd, flags, iov, iovcnt, lenp, connid) {
        let dst = null;
        try {
          dst = endpoints.isNull() ? null : endpoints.add(24).readPointer();
        } catch (e) { dst = null; }
        if (dst !== null && remoteOrNot(dst)) { denied += 1; return failErrno(); }
        return origConnectx(fd, endpoints, associd, flags, iov, iovcnt, lenp, connid);
      }, 'int', ['int', 'pointer', 'uint', 'uint', 'pointer', 'uint', 'pointer', 'pointer']));
  }
  send('NETBLOCK armed connect=' + (connectFn !== null) + ' connectx=' + (connectxFn !== null));
}

send('ARMED ' + hooked + ' hooks');

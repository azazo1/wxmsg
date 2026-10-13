//! SQLCipher 页解密.
//!
//! 微信的库是 SQLCipher 4: AES-256-CBC, 页 4096 字节, 每页末尾 80 字节放 IV(16) 与
//! HMAC-SHA512(64); 密钥是每个库独立的 32 字节原始值, **不经过 PBKDF2 作用于页数据**。
//!
//! 日常读库走 [`crate::store`] 的 SQLCipher 连接即可。本模块面向 SQLCipher 覆盖不到的
//! 场合: 解析 WAL 里被取代的历史页面版本, 以及重建一份明文副本用于对照检查。
//!
//! 页布局 (页号从 1 开始):
//!
//! ```text
//! 页 1:   [盐值 16][密文 ...][IV 16][HMAC 64]
//! 其它页: [密文 ...][IV 16][HMAC 64]
//! ```
//!
//! - 密文长度 = 页大小 - 保留区, 页 1 再扣掉 16 字节盐值
//! - HMAC-SHA512 的输入 = (页 1 的盐值) + 密文 + 页号的小端 4 字节
//! - mac key = PBKDF2-HMAC-SHA512(原始密钥, 盐值每字节异或 0x3a, 2 轮, 32 字节)
//! - 页 1 明文的前 16 字节固定是 `SQLite format 3\0`, 不参与加密, 解密时补回

use std::path::Path;

use aes::Aes256;
use aes::cipher::block_padding::NoPadding;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha512;

use crate::error::{CoreError, Result};

pub mod plaintext;

pub use plaintext::{RebuildReport, page_count, read_page, rebuild_plaintext};

type Aes256CbcDec = cbc::Decryptor<Aes256>;
type HmacSha512 = Hmac<Sha512>;

/// 盐值与 IV 的长度.
pub const SALT_SIZE: usize = 16;

/// SQLite 文件头魔数, 页 1 明文的前 16 字节.
const SQLITE_HEADER: [u8; SALT_SIZE] = *b"SQLite format 3\0";

/// 原始密钥长度.
pub const KEY_SIZE: usize = 32;

/// mac key 的派生轮数.
///
/// SQLCipher 在收到原始密钥时跳过页数据的 KDF, 但仍要用 PBKDF2 派生 HMAC 密钥,
/// 且轮数固定为 2。
const MAC_KEY_ITERATIONS: u32 = 2;

/// 盐值派生 mac key 时的异或常量.
const MAC_SALT_XOR: u8 = 0x3a;

/// 页解密错误.
#[derive(Debug, thiserror::Error)]
pub enum CipherError {
    /// 密文长度不是分组大小的整数倍.
    #[error("密文长度不是 16 字节的整数倍")]
    BlockSize,

    /// 密钥长度不合法.
    #[error("密钥长度不是 32 字节")]
    KeyLength,

    /// HMAC 与页内容对不上.
    #[error("页 {page} 的 HMAC 校验失败, 密钥与库不匹配或数据已损坏")]
    MacMismatch {
        /// 页号.
        page: u64,
    },
}

/// 加密参数.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CipherParams {
    page_size: usize,
    reserve: usize,
}

impl CipherParams {
    /// 微信当前使用的参数.
    pub fn wechat() -> Self {
        Self {
            page_size: 4096,
            reserve: 80,
        }
    }

    /// 自定义参数.
    pub fn new(page_size: usize, reserve: usize) -> Self {
        Self { page_size, reserve }
    }

    /// 页大小.
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// 每页末尾的保留区长度.
    pub fn reserve(&self) -> usize {
        self.reserve
    }

    /// 页内参与加密的字节数 (不含保留区).
    pub fn payload_size(&self) -> usize {
        self.page_size - self.reserve
    }
}

impl Default for CipherParams {
    fn default() -> Self {
        Self::wechat()
    }
}

/// 一个库的页解密器.
///
/// 盐值来自库文件的前 16 字节, 每个库各不相同, 因此解密器与库是一一对应的。
#[derive(Clone)]
pub struct PageCipher {
    key: [u8; KEY_SIZE],
    mac_key: [u8; KEY_SIZE],
    params: CipherParams,
}

impl PageCipher {
    /// 用密钥与盐值构造解密器.
    pub fn new(key: [u8; KEY_SIZE], salt: [u8; SALT_SIZE], params: CipherParams) -> Self {
        let mut mac_salt = [0u8; SALT_SIZE];
        for (target, source) in mac_salt.iter_mut().zip(salt.iter()) {
            *target = source ^ MAC_SALT_XOR;
        }
        let mut mac_key = [0u8; KEY_SIZE];
        pbkdf2::pbkdf2_hmac::<Sha512>(&key, &mac_salt, MAC_KEY_ITERATIONS, &mut mac_key);
        Self {
            key,
            mac_key,
            params,
        }
    }

    /// 从库文件读取盐值并构造解密器.
    pub fn from_db_file(
        path: impl AsRef<Path>,
        key: [u8; KEY_SIZE],
        params: CipherParams,
    ) -> Result<Self> {
        Ok(Self::new(key, read_salt(path)?, params))
    }

    /// 加密参数.
    pub fn params(&self) -> CipherParams {
        self.params
    }

    /// 解密一页, 返回长度为页大小的明文, 保留区补零.
    ///
    /// `raw` 必须正好是一页的字节数; `page_number` 从 1 开始。
    pub fn decrypt_page(&self, page_number: u64, raw: &[u8]) -> Result<Vec<u8>> {
        let params = self.params;
        if raw.len() != params.page_size {
            return Err(CoreError::PageSize {
                page: page_number,
                expected: params.page_size,
                actual: raw.len(),
            });
        }

        let payload_end = params.payload_size();
        // 页 1 的前 16 字节是盐值, 密文从第 16 字节开始。
        let prefix = if page_number == 1 { SALT_SIZE } else { 0 };
        let cipher_text = &raw[prefix..payload_end];

        self.verify_hmac(page_number, raw, prefix)?;

        let iv = &raw[payload_end..payload_end + SALT_SIZE];
        let mut buffer = cipher_text.to_vec();
        let decryptor =
            Aes256CbcDec::new_from_slices(&self.key, iv).map_err(|_| CipherError::KeyLength)?;
        let plain = decryptor
            .decrypt_padded::<NoPadding>(&mut buffer)
            .map_err(|_| CipherError::BlockSize)?;

        let mut page = Vec::with_capacity(params.page_size);
        if page_number == 1 {
            page.extend_from_slice(&SQLITE_HEADER);
        }
        page.extend_from_slice(plain);
        // 保留区在明文页里不使用, 补零即可, SQLite 会按文件头第 20 字节的保留区长度跳过。
        page.resize(params.page_size, 0);
        Ok(page)
    }

    /// 校验一页的 HMAC.
    ///
    /// SQLCipher 的校验范围是**密文加上 IV**, 末尾再接页号的小端 4 字节。
    /// 页 1 的盐值本身不参与校验 (它只用于派生密钥)。
    fn verify_hmac(&self, page_number: u64, raw: &[u8], prefix: usize) -> Result<()> {
        let params = self.params;
        let payload_end = params.payload_size();
        let mut mac =
            HmacSha512::new_from_slice(&self.mac_key).map_err(|_| CipherError::KeyLength)?;
        mac.update(&raw[prefix..payload_end + SALT_SIZE]);
        mac.update(&(page_number as u32).to_le_bytes());
        let expected = &raw[payload_end + SALT_SIZE..params.page_size];
        mac.verify_slice(expected)
            .map_err(|_| CipherError::MacMismatch { page: page_number })?;
        Ok(())
    }
}

/// 读取库文件前 16 字节的盐值.
pub fn read_salt(path: impl AsRef<Path>) -> Result<[u8; SALT_SIZE]> {
    let path = path.as_ref();
    let mut file = std::fs::File::open(path).map_err(|source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut salt = [0u8; SALT_SIZE];
    std::io::Read::read_exact(&mut file, &mut salt).map_err(|source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(salt)
}

/// 从十六进制字符串解析密钥.
pub fn key_from_hex(text: &str) -> Result<[u8; KEY_SIZE]> {
    let bytes = crate::hex::decode(text).ok_or(CipherError::KeyLength)?;
    if bytes.len() != KEY_SIZE {
        return Err(CipherError::KeyLength.into());
    }
    let mut key = [0u8; KEY_SIZE];
    key.copy_from_slice(&bytes);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_excludes_reserved_area() {
        let params = CipherParams::wechat();
        assert_eq!(params.page_size(), 4096);
        assert_eq!(params.reserve(), 80);
        assert_eq!(params.payload_size(), 4016);
    }

    #[test]
    fn mac_key_depends_on_salt() {
        let key = [7u8; KEY_SIZE];
        let first = PageCipher::new(key, [1u8; SALT_SIZE], CipherParams::wechat());
        let second = PageCipher::new(key, [2u8; SALT_SIZE], CipherParams::wechat());
        assert_ne!(first.mac_key, second.mac_key);
    }

    #[test]
    fn parses_key_hex() {
        assert!(key_from_hex(&"ab".repeat(32)).is_ok());
        assert!(key_from_hex(&"ab".repeat(8)).is_err());
        assert!(key_from_hex("xyz").is_err());
    }

    #[test]
    fn rejects_wrong_page_length() {
        let cipher = PageCipher::new([0u8; KEY_SIZE], [0u8; SALT_SIZE], CipherParams::wechat());
        let error = cipher.decrypt_page(1, &[0u8; 100]).unwrap_err();
        assert!(matches!(error, CoreError::PageSize { .. }));
    }
}

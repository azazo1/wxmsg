//! 核心库错误类型.
//!
//! 每个变体都保留足够的上下文: 出错的文件路径, 或无法解密的库名.
//! 密钥错误与文件损坏要区分开, 否则调用方无法判断是该重新抓取密钥还是该换一份数据.

use std::path::PathBuf;

/// 核心库错误.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// 打开数据库失败.
    #[error("打开数据库失败: {path}")]
    Open {
        /// 数据库路径.
        path: PathBuf,
        /// 底层错误.
        #[source]
        source: rusqlite::Error,
    },

    /// 密钥与库不匹配.
    #[error("密钥不匹配: {name} 无法用提供的密钥解密")]
    WrongKey {
        /// 库名或路径.
        name: String,
    },

    /// 其它数据库错误.
    #[error("数据库错误")]
    Sqlite(#[from] rusqlite::Error),

    /// 文件读写失败.
    #[error("文件读写失败: {path}")]
    Io {
        /// 文件路径.
        path: PathBuf,
        /// 底层错误.
        #[source]
        source: std::io::Error,
    },

    /// 密钥文件无法解析.
    #[error("密钥文件无法解析: {path}")]
    KeyFile {
        /// 密钥文件路径.
        path: PathBuf,
        /// 底层错误.
        #[source]
        source: serde_json::Error,
    },

    /// 密钥文件中的密钥长度不是 32 字节.
    #[error("密钥长度不是 32 字节: {name}")]
    KeyLength {
        /// 库名或键名.
        name: String,
    },

    /// 找不到对应的密钥.
    #[error("找不到密钥: {0}")]
    KeyMissing(String),

    /// 正文解压失败.
    #[error("正文解压失败")]
    Decompress(#[source] std::io::Error),

    /// 页长度与加密参数不符.
    #[error("页 {page} 长度异常: 期望 {expected} 字节, 实际 {actual} 字节")]
    PageSize {
        /// 页号.
        page: u64,
        /// 按参数应有的长度.
        expected: usize,
        /// 实际长度.
        actual: usize,
    },

    /// 页解密失败.
    #[error("{0}")]
    Cipher(#[from] crate::crypto::CipherError),

    /// WAL 解析失败.
    #[error("WAL 解析失败: {0}")]
    Wal(#[from] crate::wal::WalError),

    /// 记录解码失败.
    #[error("记录解码失败: {0}")]
    Record(String),

    /// 页内容不足.
    #[error("页 {page} 内容不足: 需要 {needed} 字节, 实际 {actual} 字节")]
    PageTooShort {
        /// 页号.
        page: u64,
        /// 需要的长度.
        needed: usize,
        /// 实际长度.
        actual: usize,
    },

    /// 期望叶子表页, 但页类型不符.
    #[error("页 {page} 不是叶子表页, 页类型为 {page_type}")]
    NotLeafPage {
        /// 页号.
        page: u64,
        /// 实际的页类型.
        page_type: u8,
    },

    /// 期望内部表页, 但页类型不符.
    #[error("页 {page} 不是内部表页, 页类型为 {page_type}")]
    NotInteriorPage {
        /// 页号.
        page: u64,
        /// 实际的页类型.
        page_type: u8,
    },
}

/// 核心库结果类型.
pub type Result<T> = std::result::Result<T, CoreError>;

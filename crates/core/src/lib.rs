//! wxmsg 核心库.
//!
//! 负责微信本地存储的读取, 归档与撤回还原. 本库不依赖界面, 也不修改微信自己的数据目录;
//! 需要读取真实数据时, 调用方应先把库与其 WAL 复制到工作目录再打开.
//!
//! 有关微信的存储格式, 密钥来源与撤回行为的完整结论, 见项目 `docs/handover.md`.

pub mod archive;
pub mod btree;
pub mod chat;
pub mod config;
pub mod contact;
pub mod crypto;
pub mod error;
pub mod export;
mod hex;
pub mod image;
pub mod io;
pub mod keys;
pub mod media;
pub mod message;
pub mod recall;
pub mod store;
pub mod verify;
pub mod wal;

pub use chat::{ChatEntry, list_chats, message_table_name, quote_identifier};
pub use error::{CoreError, Result};
pub use keys::{KeyEntry, KeyStore};
pub use message::{RawMessage, recent_messages};
pub use store::{EncryptedStore, copy_database};

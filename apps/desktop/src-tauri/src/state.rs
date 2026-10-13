//! 界面用的数据访问.
//!
//! 界面与命令行共用 `wxmsg-core`, 因此"怎么解页, 怎么找回"只有一份实现。这里只负责把核心库的
//! 结果整理成界面要的形状, 以及保管当前选中的账号与库。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use wxmsg_core::config::Config;
use wxmsg_core::contact::ContactBook;
use wxmsg_core::crypto::CipherParams;
use wxmsg_core::recall;
use wxmsg_core::wal::Wal;
use wxmsg_core::{EncryptedStore, KeyStore, paths};

use crate::settings::Settings;

/// 共享状态.
pub struct AppState {
    /// 数据目录.
    pub data_dir: PathBuf,
    /// 归档目录.
    pub archive_dir: PathBuf,
    /// 账号目录 (可能探测不到).
    pub account: Option<PathBuf>,
    /// 当前使用的消息库.
    pub database: Option<PathBuf>,
    /// 密钥.
    pub keys: KeyStore,
    /// 联系人.
    pub contacts: ContactBook,
    /// 设置.
    pub settings: Mutex<Settings>,
}

impl AppState {
    /// 按默认位置装配.
    pub fn load(data_dir: PathBuf) -> Self {
        let config = Config::default_path()
            .map(|path| Config::load(&path).unwrap_or_default())
            .unwrap_or_default();
        let settings = Settings::load(&data_dir);

        let account = config
            .source_dir()
            .map(|dir| paths::normalize_source(&dir))
            .or_else(|| paths::account_dir().ok());
        let database = account
            .as_deref()
            .and_then(|account| paths::account_message_database(account).ok());

        let archive_dir = config
            .archive_dir()
            .or_else(|| paths::default_archive().ok())
            .unwrap_or_else(|| data_dir.join("archive"));

        let keys_path = config.keys_file().unwrap_or_else(default_keys_path);
        let keys = KeyStore::load(&keys_path).unwrap_or_else(|error| {
            tracing::warn!(path = %keys_path.display(), %error, "密钥文件读不动, 先当作没有密钥");
            KeyStore::default()
        });
        let contacts = load_contacts(account.as_deref(), &keys);

        tracing::info!(
            account = ?account.as_deref().map(|path| path.display().to_string()),
            database = ?database.as_deref().map(|path| path.display().to_string()),
            keys = keys.len(),
            contacts = contacts.len(),
            "已装配状态"
        );

        Self {
            data_dir,
            archive_dir,
            account,
            database,
            keys,
            contacts,
            settings: Mutex::new(settings),
        }
    }

    /// 打开当前消息库.
    pub fn store(&self) -> Result<EncryptedStore, String> {
        let path = self
            .database
            .as_deref()
            .ok_or_else(|| "还没有找到微信的消息库".to_owned())?;
        EncryptedStore::open_with_keys(path, &self.keys).map_err(|error| error.to_string())
    }

    /// 一组设置.
    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    /// 改一组设置.
    pub fn update_settings(&self, next: Settings) {
        if let Ok(mut settings) = self.settings.lock() {
            *settings = next.clone();
        }
        if let Err(error) = next.save(&self.data_dir) {
            tracing::warn!(%error, "设置写盘失败");
        }
    }

    /// 归档里现有的快照数量与最新时间.
    pub fn archive_summary(&self) -> (usize, Option<i64>) {
        let Ok(archive) = wxmsg_core::archive::Archive::open(&self.archive_dir) else {
            return (0, None);
        };
        match archive.list() {
            Ok(times) => (times.len(), times.last().copied()),
            Err(_) => (0, None),
        }
    }
}

/// 密钥文件的默认位置: 先看仓库内的约定位置, 再看用户数据目录.
fn default_keys_path() -> PathBuf {
    let local = paths::default_keys();
    if local.is_file() {
        return local;
    }
    if let Some(home) = paths::home_dir() {
        let shared = home.join("Library/Application Support/wxmsg/all_keys.json");
        if shared.is_file() {
            return shared;
        }
    }
    local
}

/// 读联系人; 读不到就只用 wxid.
fn load_contacts(account: Option<&Path>, keys: &KeyStore) -> ContactBook {
    let Some(account) = account else {
        return ContactBook::default();
    };
    match wxmsg_core::contact::find_database(account, "contact/contact.db") {
        Ok(path) => ContactBook::load(&path, keys).unwrap_or_default(),
        Err(error) => {
            tracing::debug!(%error, "没有联系人库");
            ContactBook::default()
        }
    }
}

/// 一个会话.
#[derive(Debug, Serialize)]
pub struct ChatDto {
    /// 内部 wxid.
    pub username: String,
    /// 显示名.
    pub display_name: String,
    /// 消息条数.
    pub messages: i64,
    /// 最近一条消息时间.
    pub last_time: Option<i64>,
}

/// 一条消息.
#[derive(Debug, Serialize)]
pub struct MessageDto {
    /// 会话内编号.
    pub local_id: i64,
    /// 时间.
    pub create_time: i64,
    /// 发送者显示名.
    pub sender: String,
    /// 类型的中文说法.
    pub kind: String,
    /// 正文.
    pub text: String,
    /// 是不是找回出来的.
    pub revoked: bool,
}

/// 列出会话.
pub fn chats(state: &AppState) -> Result<Vec<ChatDto>, String> {
    let store = state.store()?;
    let chats = wxmsg_core::list_chats(&store).map_err(|error| error.to_string())?;
    Ok(chats
        .into_iter()
        .filter(|chat| chat.message_count > 0)
        .map(|chat| ChatDto {
            display_name: state.contacts.display_name(&chat.username),
            username: chat.username,
            messages: chat.message_count,
            last_time: chat.last_message_time,
        })
        .collect())
}

/// 列出某个会话的消息, 最新的 `limit` 条按时间正序返回.
pub fn messages(state: &AppState, chat: &str, limit: usize) -> Result<Vec<MessageDto>, String> {
    let store = state.store()?;
    let mut messages =
        wxmsg_core::recent_messages(&store, chat, limit).map_err(|error| error.to_string())?;
    messages.reverse();

    // 发送者编号要查 Name2Id, 因此这里建一份编号到 wxid 的表。
    let names: std::collections::BTreeMap<i64, String> = wxmsg_core::list_chats(&store)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|chat| (chat.session_id, chat.username))
        .collect();

    Ok(messages
        .iter()
        .map(|message| to_dto(state, &names, chat, message, false))
        .collect())
}

/// 找回某个会话里被撤回 (被删) 的消息.
pub fn revoked(state: &AppState, chat: &str) -> Result<Vec<MessageDto>, String> {
    let store = state.store()?;
    let database = state
        .database
        .as_deref()
        .ok_or_else(|| "还没有找到微信的消息库".to_owned())?;
    let key = *state
        .keys
        .require(&database.display().to_string())
        .map_err(|error| error.to_string())?;

    let mut wal_path = database.as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal = Wal::open_if_present(PathBuf::from(wal_path)).map_err(|error| error.to_string())?;

    let report = recall::recover(
        database,
        wal.as_ref(),
        key,
        CipherParams::wechat(),
        &store,
        Some(chat),
    )
    .map_err(|error| error.to_string())?;
    tracing::info!(
        chat,
        found = report.messages.len(),
        pages = report.pages,
        "界面里跑完一次找回"
    );

    let names: std::collections::BTreeMap<i64, String> = wxmsg_core::list_chats(&store)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|chat| (chat.session_id, chat.username))
        .collect();

    Ok(report
        .messages
        .iter()
        .map(|item| to_dto(state, &names, chat, &item.message, true))
        .collect())
}

/// 把核心库的消息整理成界面用的形状.
fn to_dto(
    state: &AppState,
    names: &std::collections::BTreeMap<i64, String>,
    chat: &str,
    message: &wxmsg_core::RawMessage,
    revoked: bool,
) -> MessageDto {
    let sender = message
        .sender_id
        .and_then(|id| names.get(&id))
        .map(|username| state.contacts.display_name(username))
        .or_else(|| {
            message
                .sender()
                .ok()
                .flatten()
                .map(|username| state.contacts.display_name(&username))
        })
        .unwrap_or_else(|| state.contacts.display_name(chat));

    MessageDto {
        local_id: message.local_id,
        create_time: message.create_time,
        sender,
        kind: kind_name(message.real_type()).to_owned(),
        text: message.text().unwrap_or_default(),
        revoked,
    }
}

/// 消息类型的中文说法.
fn kind_name(real_type: i64) -> &'static str {
    match real_type {
        1 => "文本",
        3 => "图片",
        34 => "语音",
        43 => "视频",
        47 => "表情",
        48 => "位置",
        49 => "应用消息",
        50 => "通话",
        10000 => "系统",
        _ => "其它",
    }
}

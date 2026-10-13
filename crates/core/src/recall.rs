//! 从 WAL 与主库里找回被撤回 (被删除) 的消息.
//!
//! 撤回会把消息行从表里删掉, 但**删除之前那个页版本**还留在原处: 要么在 WAL 的帧里, 要么在
//! 主库文件中尚未被回收的页内空隙里。于是
//!
//! 1. 用 SQLite 读出每张消息表的行号集合 (当前还存在的行), 以及表结构
//! 2. 用 [`crate::btree`] 定位每张表的叶页
//! 3. 把主库的每个叶页、以及 WAL 里承载这些叶页的每一帧都解成行
//! 4. 在历史里出现过、但当前行集合里没有的行号, 就是被删掉的消息
//!
//! 边界: 叶页集合以**当前**结构为准, 因此很旧的历史可能因为页号变动而漏掉; 载荷溢出到其它
//! 页的长消息只能解出页内那一段。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::btree::{self, PageSource};
use crate::chat::{message_table_name, quote_identifier};
use crate::crypto::{CipherParams, KEY_SIZE, PageCipher, PageReader};
use crate::error::Result;
use crate::message::RawMessage;
use crate::store::EncryptedStore;
use crate::wal::Wal;

/// 一行是在哪里找到的.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundIn {
    /// 主库文件中该页的当前内容 (页内可能残留更早被删的单元).
    DbFile,
    /// WAL 的某一代的某一帧.
    Wal {
        /// 第几代 (按文件里的位置).
        generation: usize,
        /// 帧序号.
        frame: u32,
    },
}

/// 一条被找回来的消息.
#[derive(Debug, Clone)]
pub struct RecoveredMessage {
    /// 所在的消息表.
    pub table: String,
    /// 消息本体.
    pub message: RawMessage,
    /// 疑似对应的撤回通知.
    pub notice: Option<RawMessage>,
    /// 载荷是否溢出, 内容可能不完整.
    pub truncated: bool,
    /// 这一行是在哪里找到的.
    pub found_in: FoundIn,
}

/// 找回结果.
#[derive(Debug, Clone, Default)]
pub struct RecallReport {
    /// 参与检查的消息表数量.
    pub tables: usize,
    /// 解过的页版本数量.
    pub pages: usize,
    /// 从历史页里解出的行数.
    pub historical_rows: usize,
    /// 载荷溢出而可能不完整的行数.
    pub truncated_rows: usize,
    /// 找回来的消息, 按时间正序.
    pub messages: Vec<RecoveredMessage>,
}

/// 一张消息表里我们关心的列位置.
struct TableInfo {
    name: String,
    root_page: u32,
    /// 表的列数, 用于判断某个页版本是否真的属于这张表.
    columns: usize,
    local_id: usize,
    create_time: usize,
    local_type: usize,
    content: usize,
    content_flag: Option<usize>,
}

/// 主库里的一行历史版本.
struct HistoricalRow {
    message: RawMessage,
    truncated: bool,
    found_in: FoundIn,
}

/// 从主库文件读页的来源.
struct DbPageSource<'a> {
    reader: &'a PageReader,
}

impl PageSource for DbPageSource<'_> {
    fn read_page(&self, page_number: u32) -> Result<Vec<u8>> {
        self.reader.read(page_number as u64)
    }
}

/// 从库与 WAL 里找回被删除的消息.
///
/// `chat` 给出内部 wxid 时只检查该会话的表。
pub fn recover(
    db_path: &Path,
    wal: Option<&Wal>,
    key: [u8; KEY_SIZE],
    params: CipherParams,
    store: &EncryptedStore,
    chat: Option<&str>,
) -> Result<RecallReport> {
    let mut report = RecallReport::default();

    let wanted = match chat {
        Some(username) => vec![message_table_name(username)],
        None => message_tables(store)?,
    };
    let mut tables = Vec::new();
    for name in wanted {
        if let Some(info) = table_info(store, &name)? {
            tables.push(info);
        }
    }
    report.tables = tables.len();
    if tables.is_empty() {
        return Ok(report);
    }

    let cipher = PageCipher::from_db_file(db_path, key, params)?;
    let reader = PageReader::open(db_path, cipher.clone())?;

    // 叶页 -> 表下标. 页号会随表变化, 这里以当前结构为准。
    let source = DbPageSource { reader: &reader };
    let mut leaf_owner: BTreeMap<u32, usize> = BTreeMap::new();
    for (index, table) in tables.iter().enumerate() {
        for page in btree::leaf_pages(&source, table.root_page)? {
            leaf_owner.insert(page, index);
        }
    }
    tracing::info!(
        tables = tables.len(),
        leaf_pages = leaf_owner.len(),
        "已定位消息表的叶页"
    );

    // 当前还存在的行号与撤回通知.
    let mut current: Vec<BTreeSet<i64>> = Vec::with_capacity(tables.len());
    let mut notices: Vec<Vec<RawMessage>> = Vec::with_capacity(tables.len());
    for table in &tables {
        current.push(current_rowids(store, table)?);
        notices.push(current_notices(store, table)?);
    }

    let usable_size = params.payload_size();
    let mut historical: BTreeMap<(usize, i64), HistoricalRow> = BTreeMap::new();

    // 一、主库里的页: 页内可能还留着更早被删的单元。
    for (&page_number, &index) in &leaf_owner {
        let page = reader.read(page_number as u64)?;
        report.pages += 1;
        absorb(
            &page,
            page_number as u64,
            index,
            &tables[index],
            usable_size,
            FoundIn::DbFile,
            &mut historical,
            &mut report,
        )?;
    }

    // 二、WAL 各代的帧: 每一帧都是一个更早的页版本。
    if let Some(wal) = wal {
        for (generation, frame) in wal.all_frames() {
            let Some(&index) = leaf_owner.get(&frame.page_number) else {
                continue;
            };
            let page = cipher.decrypt_page(frame.page_number as u64, &frame.page)?;
            report.pages += 1;
            absorb(
                &page,
                frame.page_number as u64,
                index,
                &tables[index],
                usable_size,
                FoundIn::Wal {
                    generation,
                    frame: frame.index,
                },
                &mut historical,
                &mut report,
            )?;
        }
    }

    // 历史里有、当前没有的行号就是被删掉的。
    for ((index, rowid), row) in historical {
        if current[index].contains(&rowid) {
            continue;
        }
        let notice = pair_notice(&notices[index], &row.message);
        report.messages.push(RecoveredMessage {
            table: tables[index].name.clone(),
            message: row.message,
            notice,
            truncated: row.truncated,
            found_in: row.found_in,
        });
    }
    report.messages.sort_by_key(|item| item.message.create_time);
    Ok(report)
}

/// 把一页版本里的行收进历史表.
#[allow(clippy::too_many_arguments)]
fn absorb(
    page: &[u8],
    page_number: u64,
    table_index: usize,
    table: &TableInfo,
    usable_size: usize,
    origin: FoundIn,
    historical: &mut BTreeMap<(usize, i64), HistoricalRow>,
    report: &mut RecallReport,
) -> Result<()> {
    let cells = btree::leaf_cells(page, page_number, usable_size)?;
    for cell in cells {
        // 溢出载荷只解得出页内那一段, 完整解码可能失败, 跳过即可。
        let Ok(values) = btree::decode_record(&cell.payload) else {
            report.truncated_rows += 1;
            continue;
        };
        let Some(message) = to_message(table, cell.rowid, &values) else {
            continue;
        };
        if cell.overflow {
            report.truncated_rows += 1;
        }
        report.historical_rows += 1;
        // 先到的版本更接近原始内容 (主库最早, 之后按文件里的帧序), 因此只记第一次见到的。
        historical
            .entry((table_index, cell.rowid))
            .or_insert(HistoricalRow {
                message,
                truncated: cell.overflow,
                found_in: origin,
            });
    }
    Ok(())
}

/// 找出库里全部消息表 (`Msg_` 前缀).
fn message_tables(store: &EncryptedStore) -> Result<Vec<String>> {
    let mut statement = store.connection().prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'Msg\\_%' ESCAPE '\\' ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

/// 读出某张表的根页号与关心的列位置.
fn table_info(store: &EncryptedStore, table: &str) -> Result<Option<TableInfo>> {
    let root_page: Option<i64> = store
        .connection()
        .query_row(
            "SELECT rootpage FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .ok();
    let Some(root_page) = root_page else {
        return Ok(None);
    };

    let mut columns = Vec::new();
    {
        let sql = format!("PRAGMA table_info({})", quote_identifier(table));
        let mut statement = store.connection().prepare(&sql)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
        for row in rows {
            columns.push(row?);
        }
    }
    let position = |name: &str| columns.iter().position(|column| column == name);

    let (Some(local_id), Some(create_time), Some(local_type), Some(content)) = (
        position("local_id"),
        position("create_time"),
        position("local_type"),
        position("message_content"),
    ) else {
        tracing::debug!(table, "缺少需要的列, 跳过");
        return Ok(None);
    };

    Ok(Some(TableInfo {
        name: table.to_owned(),
        root_page: root_page as u32,
        columns: columns.len(),
        local_id,
        create_time,
        local_type,
        content,
        content_flag: position("WCDB_CT_message_content"),
    }))
}

/// 当前表里全部行号.
fn current_rowids(store: &EncryptedStore, table: &TableInfo) -> Result<BTreeSet<i64>> {
    let sql = format!("SELECT local_id FROM {}", quote_identifier(&table.name));
    let mut statement = store.connection().prepare(&sql)?;
    let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
    let mut ids = BTreeSet::new();
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// 当前表里的撤回通知 (真实类型 10000).
fn current_notices(store: &EncryptedStore, table: &TableInfo) -> Result<Vec<RawMessage>> {
    let columns = if table.content_flag.is_some() {
        format!(
            "{}, {}, {}, {}, {}",
            quote_identifier("local_id"),
            quote_identifier("local_type"),
            quote_identifier("create_time"),
            quote_identifier("message_content"),
            quote_identifier("WCDB_CT_message_content")
        )
    } else {
        format!(
            "{}, {}, {}, {}, NULL",
            quote_identifier("local_id"),
            quote_identifier("local_type"),
            quote_identifier("create_time"),
            quote_identifier("message_content")
        )
    };
    let sql = format!(
        "SELECT {columns} FROM {} WHERE ({} & 4294967295) = 10000 ORDER BY create_time",
        quote_identifier(&table.name),
        quote_identifier("local_type")
    );

    let mut statement = store.connection().prepare(&sql)?;
    let rows = statement.query_map([], |row| {
        Ok(RawMessage {
            local_id: row.get(0)?,
            local_type: row.get(1)?,
            create_time: row.get(2)?,
            content: read_bytes(row, 3)?,
            content_flag: row.get(4)?,
        })
    })?;

    let mut notices = Vec::new();
    for row in rows {
        notices.push(row?);
    }
    Ok(notices)
}

/// 按 SQLite 的动态类型读一列字节.
fn read_bytes(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    use rusqlite::types::ValueRef;
    Ok(match row.get_ref(index)? {
        ValueRef::Null => None,
        ValueRef::Integer(value) => Some(value.to_string().into_bytes()),
        ValueRef::Real(value) => Some(value.to_string().into_bytes()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(bytes.to_vec()),
    })
}

/// 把解码出来的列组装成一条消息.
fn to_message(table: &TableInfo, rowid: i64, values: &[btree::Value]) -> Option<RawMessage> {
    // 列数与表结构对不上, 说明这一页其实不属于这张表 (页号被复用了).
    if values.len() != table.columns {
        return None;
    }
    // `local_id` 是 INTEGER PRIMARY KEY, SQLite 在记录里给它存的是 NULL, 真实值就是行号。
    let local_id = match values.get(table.local_id) {
        Some(btree::Value::Null) | None => rowid,
        Some(value) => value.as_integer()?,
    };
    let create_time = values.get(table.create_time)?.as_integer().unwrap_or(0);
    let local_type = values.get(table.local_type)?.as_integer().unwrap_or(0);
    let content = values
        .get(table.content)
        .and_then(btree::Value::as_bytes)
        .map(<[u8]>::to_vec);
    let content_flag = table
        .content_flag
        .and_then(|index| values.get(index))
        .and_then(btree::Value::as_integer);

    Some(RawMessage {
        local_id,
        local_type,
        create_time,
        content,
        content_flag,
    })
}

/// 撤回通知与原消息的时间差上限.
///
/// 撤回是"先删原消息, 再插入通知", 两者间隔通常只有一两秒。窗口放这么宽只是留出时钟与写入
/// 顺序的余量; 超出窗口就不认为这两条属于同一次撤回, 免得给出误导性的配对。
const NOTICE_WINDOW_SECONDS: i64 = 300;

/// 给一条被删消息找最可能对应的撤回通知.
fn pair_notice(notices: &[RawMessage], message: &RawMessage) -> Option<RawMessage> {
    notices
        .iter()
        .filter(|notice| {
            notice.create_time >= message.create_time
                && notice.create_time - message.create_time <= NOTICE_WINDOW_SECONDS
        })
        .min_by_key(|notice| notice.create_time - message.create_time)
        .cloned()
}

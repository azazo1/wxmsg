//! 导出会话.
//!
//! 一条消息在三种格式里都先归一到同一个 [`Rendered`] 结构, 再按格式写出。这样"什么算图片,
//! 什么算文件"的判断只写一遍, 三种输出不会互相跑偏。
//!
//! 撤回 (被删) 的消息可以和正常消息**按时间合并**写进去, 并标上 `[撤回]`, 因此导出的记录能
//! 还原成会话当时的完整样子。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{Local, TimeZone};
use serde_json::json;

use crate::chat::list_chats;
use crate::contact::ContactBook;
use crate::error::{CoreError, Result};
use crate::media::MediaLocator;
use crate::message::{RawMessage, for_each_message};
use crate::recall::RecoveredMessage;
use crate::store::EncryptedStore;

/// 导出格式.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 便于阅读的 Markdown.
    Markdown,
    /// 便于程序处理的 JSON.
    Json,
    /// 便于表格软件打开的 CSV.
    Csv,
}

impl Format {
    /// 从命令行文本解析.
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "json" => Some(Self::Json),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }

    /// 扩展名.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }
}

/// 导出选项.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// 输出格式.
    pub format: Format,
    /// 输出目录.
    pub output: PathBuf,
    /// 账号目录; 给了才尝试导出媒体文件.
    pub account: Option<PathBuf>,
    /// 是否把撤回 (被删) 的消息也写进去.
    pub include_revoked: bool,
}

/// 导出结果.
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    /// 导出的会话数.
    pub chats: usize,
    /// 导出的正常消息条数.
    pub messages: usize,
    /// 其中撤回 (被删) 消息的条数.
    pub revoked: usize,
    /// 复制出来的媒体文件数.
    pub media: usize,
    /// 写出的文件.
    pub files: Vec<PathBuf>,
}

/// 一条已归一的待写出消息.
struct Rendered {
    create_time: i64,
    local_id: i64,
    sender: String,
    /// 类型的中文说法.
    kind: &'static str,
    /// 正文.
    body: String,
    /// 对应的媒体源文件 (若有).
    media: Option<PathBuf>,
    /// 是不是撤回 (被删) 的消息.
    revoked: bool,
    /// 是否被截断 (载荷溢出).
    truncated: bool,
}

impl Rendered {
    /// 时间文本.
    fn time_text(&self) -> String {
        match Local.timestamp_opt(self.create_time, 0).single() {
            Some(time) => time.format("%Y-%m-%d %H:%M:%S").to_string(),
            None => self.create_time.to_string(),
        }
    }

    /// 正文, 必要时补上截断提示.
    fn body_text(&self) -> String {
        if self.truncated {
            format!("{} [载荷溢出, 内容可能不完整]", self.body)
        } else {
            self.body.clone()
        }
    }
}

/// 导出指定会话; `chats` 为空表示导出全部会话.
///
/// `revoked` 是找回模块的结果, 会按表归属与时间合并进对应会话。
pub fn export(
    store: &EncryptedStore,
    contacts: &ContactBook,
    options: &ExportOptions,
    chats: &[String],
    revoked: &[RecoveredMessage],
) -> Result<ExportReport> {
    std::fs::create_dir_all(&options.output).map_err(|source| CoreError::Io {
        path: options.output.clone(),
        source,
    })?;

    let all = list_chats(store)?;
    let selected: Vec<_> = all
        .iter()
        .filter(|chat| chats.is_empty() || chats.contains(&chat.username))
        .collect();
    if selected.is_empty() {
        return Err(CoreError::Archive(if chats.is_empty() {
            "这个库里没有会话".to_owned()
        } else {
            format!("没找到会话 {}", chats.join(", "))
        }));
    }

    let locator = options.account.as_deref().map(MediaLocator::new);
    // 发送者只给了 Name2Id 的行号, 这里换成 wxid; 每个分片的编号空间是独立的, 因此按整库建表。
    let names: BTreeMap<i64, String> = all
        .iter()
        .map(|chat| (chat.session_id, chat.username.clone()))
        .collect();
    let mut report = ExportReport::default();
    let mut used_names: BTreeMap<String, usize> = BTreeMap::new();

    for chat in selected {
        // 撤回消息按表归属, 再按时间排序, 便于与正常消息按时间合并。
        let mut pending: Vec<&RecoveredMessage> = revoked
            .iter()
            .filter(|item| item.table == chat.table_name())
            .collect();
        pending.sort_by_key(|item| (item.message.create_time, item.message.local_id));
        let mut pending = pending.into_iter().peekable();

        let display_name = contacts.display_name(&chat.username);
        let directory = options.output.join(unique_directory_name(
            &safe_name(&display_name),
            &chat.username,
            &mut used_names,
        ));
        let media_directory = directory.join("media");
        std::fs::create_dir_all(&directory).map_err(|source| CoreError::Io {
            path: directory.clone(),
            source,
        })?;

        let file = directory.join(format!("messages.{}", options.format.extension()));
        let handle = std::fs::File::create(&file).map_err(|source| CoreError::Io {
            path: file.clone(),
            source,
        })?;
        let mut writer = std::io::BufWriter::new(handle);
        let mut first_json = true;

        write_header(
            &mut writer,
            options.format,
            &display_name,
            &chat.username,
            chat.message_count,
        )
        .map_err(|source| CoreError::Io {
            path: file.clone(),
            source,
        })?;

        for_each_message(store, &chat.username, |message| {
            // 先把时间上更早的撤回消息写掉, 保证合并后仍按时间递增。
            while let Some(item) = pending.peek()
                && item.message.create_time <= message.create_time
            {
                let item = pending.next().expect("peek 之后必有值");
                if options.include_revoked {
                    let rendered =
                        render_revoked(item, &chat.username, contacts, locator.as_ref(), &names);
                    let rendered = attach_media(rendered, &media_directory, &mut report)?;
                    write_message(&mut writer, options.format, &rendered, &mut first_json)
                        .map_err(|source| CoreError::Io {
                            path: file.clone(),
                            source,
                        })?;
                    report.revoked += 1;
                }
            }

            let rendered = render(message, &chat.username, contacts, locator.as_ref(), &names);
            let rendered = attach_media(rendered, &media_directory, &mut report)?;
            write_message(&mut writer, options.format, &rendered, &mut first_json).map_err(
                |source| CoreError::Io {
                    path: file.clone(),
                    source,
                },
            )?;
            report.messages += 1;
            Ok(())
        })?;

        // 剩下的撤回消息时间都晚于最后一条正常消息。
        for item in pending {
            if options.include_revoked {
                let rendered =
                    render_revoked(item, &chat.username, contacts, locator.as_ref(), &names);
                let rendered = attach_media(rendered, &media_directory, &mut report)?;
                write_message(&mut writer, options.format, &rendered, &mut first_json).map_err(
                    |source| CoreError::Io {
                        path: file.clone(),
                        source,
                    },
                )?;
                report.revoked += 1;
            }
        }

        finish(&mut writer, options.format).map_err(|source| CoreError::Io {
            path: file.clone(),
            source,
        })?;
        writer.flush().map_err(|source| CoreError::Io {
            path: file.clone(),
            source,
        })?;

        tracing::info!(
            chat = %display_name,
            directory = %directory.display(),
            messages = chat.message_count,
            "已导出会话"
        );
        report.chats += 1;
        report.files.push(file);
    }

    Ok(report)
}

/// 把一条实时消息归一.
fn render(
    message: &RawMessage,
    chat: &str,
    contacts: &ContactBook,
    locator: Option<&MediaLocator>,
    names: &BTreeMap<i64, String>,
) -> Rendered {
    let sender = resolve_sender(message, chat, contacts, names);
    let text = message.text().unwrap_or_default();
    let local_type = message.real_type();
    // 图片的本地文件名有时直接用消息里的 md5, 因此先取出来, 免得正文被 move 掉。
    let image_digest = attribute(&text, "md5");

    let (kind, body, attachment) = match local_type {
        1 => ("文本", text, None),
        3 => ("图片", "[图片]".to_owned(), None),
        34 => ("语音", "[语音]".to_owned(), None),
        43 => {
            let digest = attribute(&text, "md5").unwrap_or_default();
            let media = locator.and_then(|locator| locator.video(message.create_time, &digest));
            ("视频", "[视频]".to_owned(), media.map(Attachment::Found))
        }
        49 => describe_app_message(&text),
        10000 => ("系统", describe_system(&text), None),
        other => ("其他", format!("[类型 {other}] {text}"), None),
    };

    let mut media = match attachment {
        // 视频封面在渲染时就能定位。
        Some(Attachment::Found(path)) => Some(path),
        // 文件附件只知道名字, 这里按"年月目录 + 名字"去找。
        Some(Attachment::Named(name)) => {
            locator.and_then(|locator| locator.file(message.create_time, &name))
        }
        None => None,
    };

    if media.is_none() && local_type == 3 {
        // 图片缩略图按"会话 + 年月 + 行号_时间"或"消息里的 md5"落在会话自己的目录里。
        media = locator.and_then(|locator| {
            locator.image(
                chat,
                message.create_time,
                message.local_id,
                image_digest.as_deref(),
            )
        });
    }

    Rendered {
        create_time: message.create_time,
        local_id: message.local_id,
        sender,
        kind,
        body: collapse(&body),
        media,
        revoked: false,
        truncated: false,
    }
}

/// 判断一条消息是谁发的.
///
/// 优先用 `real_sender_id` 查本分片的 `Name2Id`, 这是唯一可靠的来源: 一对一聊天的正文里
/// 根本没写发送者。正文前缀只在拿不到编号时兜底, 会话名是最后的回落。
fn resolve_sender(
    message: &RawMessage,
    chat: &str,
    contacts: &ContactBook,
    names: &BTreeMap<i64, String>,
) -> String {
    if let Some(id) = message.sender_id
        && let Some(username) = names.get(&id)
    {
        return contacts.display_name(username);
    }
    match message.sender() {
        Ok(Some(username)) => contacts.display_name(&username),
        _ => chat.to_owned(),
    }
}

/// 应用消息带来的附件: 要么已经是路径, 要么只知道名字.
enum Attachment {
    /// 已经定位到的文件.
    Found(PathBuf),
    /// 只有名字, 需要再按时间去找.
    Named(String),
}

/// 把一条找回的 (被删) 消息归一.
fn render_revoked(
    item: &RecoveredMessage,
    chat: &str,
    contacts: &ContactBook,
    locator: Option<&MediaLocator>,
    names: &BTreeMap<i64, String>,
) -> Rendered {
    let mut rendered = render(&item.message, chat, contacts, locator, names);
    rendered.revoked = true;
    rendered.truncated = item.truncated;
    rendered
}

/// 解析应用消息 (`local_type = 49`) 的 XML.
///
/// 返回值里的第三项是附件信息: 文件消息给出附件名, 由调用方按名字与月份去定位。
fn describe_app_message(text: &str) -> (&'static str, String, Option<Attachment>) {
    let app_type = tag_value(text, "type").unwrap_or_default();
    let title = tag_value(text, "title").unwrap_or_default();
    match app_type.as_str() {
        "6" => (
            "文件",
            title.clone(),
            if title.is_empty() {
                None
            } else {
                Some(Attachment::Named(title))
            },
        ),
        "5" => {
            let url = tag_value(text, "url").unwrap_or_default();
            ("链接", format!("{title} {url}"), None)
        }
        "57" => (
            "引用",
            tag_value(text, "title").unwrap_or_else(|| text.to_owned()),
            None,
        ),
        "33" | "36" => ("小程序", title, None),
        "62" => ("拍一拍", title, None),
        "87" => ("群公告", title, None),
        _ => (
            "应用消息",
            if title.is_empty() {
                text.to_owned()
            } else {
                title
            },
            None,
        ),
    }
}

/// 系统消息: 撤回收到的是一段 sysmsg XML, 这里取出其中的人话.
fn describe_system(text: &str) -> String {
    if text.contains("revokemsg") {
        let replaced = tag_value(text, "replacemsg").unwrap_or_default();
        if replaced.is_empty() {
            "撤回了一条消息".to_owned()
        } else {
            replaced
        }
    } else {
        text.to_owned()
    }
}

/// 把媒体文件复制到导出目录.
fn attach_media(
    mut rendered: Rendered,
    media_directory: &Path,
    report: &mut ExportReport,
) -> Result<Rendered> {
    let Some(source) = rendered.media.clone() else {
        return Ok(rendered);
    };

    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".to_owned());
    let target = media_directory.join(format!("{}_{}", rendered.local_id, safe_name(&name)));

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| CoreError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    match std::fs::copy(&source, &target) {
        Ok(_) => {
            report.media += 1;
            rendered.media = Some(target);
            Ok(rendered)
        }
        Err(error) => {
            tracing::warn!(%error, path = %source.display(), "复制媒体文件失败, 只记录原路径");
            Ok(rendered)
        }
    }
}

/// 写会话头.
fn write_header(
    writer: &mut impl Write,
    format: Format,
    display: &str,
    username: &str,
    count: i64,
) -> std::io::Result<()> {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    match format {
        Format::Markdown => writeln!(
            writer,
            "# {display}\n\n- 内部 wxid: `{username}`\n- 消息条数: {count}\n- 导出时间: {now}\n"
        ),
        Format::Json => writeln!(
            writer,
            "{{\n  \"chat\": {},\n  \"username\": {},\n  \"exported_at\": {},\n  \"messages\": [",
            json!(display),
            json!(username),
            json!(now)
        ),
        Format::Csv => writeln!(writer, "时间,行号,发送者,类型,撤回,正文,媒体"),
    }
}

/// 写一条消息.
fn write_message(
    writer: &mut impl Write,
    format: Format,
    message: &Rendered,
    first_json: &mut bool,
) -> std::io::Result<()> {
    let media_name = message
        .media
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    match format {
        Format::Markdown => {
            let media = if media_name.is_empty() {
                String::new()
            } else {
                format!("\n\n附件: [{media_name}](media/{media_name})")
            };
            writeln!(
                writer,
                "### {} {} {}{}\n\n**{}**{}\n",
                message.time_text(),
                message.sender,
                message.kind,
                if message.revoked { " [撤回]" } else { "" },
                message.body_text(),
                media
            )
        }
        Format::Json => {
            let separator = if *first_json { "" } else { ",\n" };
            *first_json = false;
            writeln!(
                writer,
                "{separator}    {{\"time\": {}, \"local_id\": {}, \"sender\": {}, \"kind\": {}, \"revoked\": {}, \"truncated\": {}, \"text\": {}, \"media\": {}}}",
                json!(message.create_time),
                message.local_id,
                json!(message.sender),
                json!(message.kind),
                message.revoked,
                message.truncated,
                json!(message.body_text()),
                json!(media_name)
            )
        }
        Format::Csv => writeln!(
            writer,
            "{},{},{},{},{},{},{}",
            csv(&message.time_text()),
            message.local_id,
            csv(&message.sender),
            csv(message.kind),
            if message.revoked { "是" } else { "否" },
            csv(&message.body_text()),
            csv(&media_name)
        ),
    }
}

/// 收尾.
fn finish(writer: &mut impl Write, format: Format) -> std::io::Result<()> {
    match format {
        Format::Markdown => writeln!(writer, "\n---\n\n由 wxmsg 导出。"),
        Format::Json => writeln!(writer, "\n  ]\n}}"),
        Format::Csv => Ok(()),
    }
}

/// CSV 字段转义.
fn csv(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// 取 XML 里某个标签的文本.
fn tag_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(decode_entities(&xml[start..end]))
}

/// 取 XML 属性值.
fn attribute(xml: &str, name: &str) -> Option<String> {
    let pattern = format!("{name}=\"");
    let start = xml.find(&pattern)? + pattern.len();
    let end = xml[start..].find('"')? + start;
    Some(xml[start..end].to_owned())
}

/// 处理几个常见的实体.
fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// 折叠空白, 让正文适合放进表格与 Markdown 段落.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 目录名里不能出现的字符.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => '_',
            other => other,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_owned();
    if trimmed.is_empty() {
        "未命名会话".to_owned()
    } else {
        trimmed
    }
}

/// 同名会话加序号, 避免互相覆盖.
fn unique_directory_name(base: &str, username: &str, used: &mut BTreeMap<String, usize>) -> String {
    match used.get(base) {
        None => {
            used.insert(base.to_owned(), 1);
            base.to_owned()
        }
        Some(count) => {
            used.insert(base.to_owned(), count + 1);
            // 加上内部 wxid 的后几位, 既区分得开又便于追查。
            let suffix: String = username
                .chars()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{base}-{suffix}")
        }
    }
}

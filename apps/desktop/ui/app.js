//! 界面逻辑.
//!
//! 不引入前端框架: 界面只有三块内容 (会话, 消息, 设置), 用原生 DOM 足够, 也省掉一整套构建链。
//! 与后端的契约只有几个命令, 都在 `src/commands.rs` 里。

const { invoke } = window.__TAURI__.core;

const state = {
  info: null,
  chats: [],
  current: "",
  messages: [],
  revokedOnly: false,
};

const elements = {
  chatList: document.getElementById("chat-list"),
  chatEmpty: document.getElementById("chat-empty"),
  chatSearch: document.getElementById("chat-search"),
  chatTitle: document.getElementById("chat-title"),
  chatSubtitle: document.getElementById("chat-subtitle"),
  messageList: document.getElementById("message-list"),
  detailEmpty: document.getElementById("detail-empty"),
  more: document.getElementById("more-button"),
  revoked: document.getElementById("show-revoked"),
  version: document.getElementById("status-version"),
  source: document.getElementById("status-source"),
  archive: document.getElementById("status-archive"),
  settings: document.getElementById("settings-dialog"),
  settingsButton: document.getElementById("settings-button"),
  settingsSave: document.getElementById("settings-save"),
  settingStartHidden: document.getElementById("setting-start-hidden"),
  settingHideDock: document.getElementById("setting-hide-dock"),
  settingLimit: document.getElementById("setting-limit"),
  settingsMeta: document.getElementById("settings-meta"),
  quit: document.getElementById("quit-button"),
};

function formatTime(seconds) {
  if (!seconds) return "";
  const date = new Date(seconds * 1000);
  const pad = (value) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

function shortCount(count) {
  if (count >= 10000) return `${Math.round(count / 1000)}k`;
  return String(count);
}

async function loadInfo() {
  state.info = await invoke("app_info");
  elements.version.textContent = `v${state.info.version}`;
  const parts = [];
  if (state.info.database) parts.push(state.info.database);
  else parts.push("没有找到微信的消息库");
  parts.push(`密钥 ${state.info.keys} 个`);
  elements.source.textContent = parts.join(" · ");
  elements.source.title = state.info.database || "";
  elements.archive.textContent = `归档 ${state.info.archive_snapshots} 份`;
  elements.archive.title = state.info.archive_dir;

  elements.settingStartHidden.checked = state.info.settings.start_hidden;
  elements.settingHideDock.checked = state.info.settings.hide_dock_when_hidden;
  elements.settingLimit.value = state.info.settings.message_limit;
  elements.settingsMeta.textContent = `数据目录 ${state.info.data_dir}`;
}

async function loadChats() {
  elements.chatEmpty.textContent = "正在读取会话…";
  try {
    state.chats = await invoke("list_chats");
  } catch (error) {
    state.chats = [];
    elements.chatEmpty.textContent = `读取会话失败: ${error}`;
    return;
  }
  elements.chatEmpty.hidden = state.chats.length > 0;
  renderChats();
  if (state.chats.length > 0) {
    const preferred = state.chats.find((chat) => chat.username === state.info?.settings.last_chat);
    await selectChat((preferred || state.chats[0]).username);
  }
}

function renderChats() {
  const keyword = elements.chatSearch.value.trim().toLowerCase();
  const list = state.chats.filter((chat) => {
    if (!keyword) return true;
    return (
      chat.display_name.toLowerCase().includes(keyword) ||
      chat.username.toLowerCase().includes(keyword)
    );
  });

  elements.chatList.replaceChildren(
    ...list.map((chat) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "chat-item";
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", String(chat.username === state.current));

      const name = document.createElement("span");
      name.className = "chat-name";
      name.textContent = chat.display_name;
      name.title = chat.username;

      const meta = document.createElement("span");
      meta.className = "chat-meta";
      meta.textContent = shortCount(chat.messages);

      item.append(name, meta);
      item.addEventListener("click", () => selectChat(chat.username));
      return item;
    }),
  );
}

async function selectChat(username) {
  if (!username) return;
  state.current = username;
  const chat = state.chats.find((entry) => entry.username === username);
  elements.chatTitle.textContent = chat ? chat.display_name : username;
  elements.chatSubtitle.textContent = `${username} · ${chat ? chat.messages : 0} 条消息`;
  renderChats();

  elements.detailEmpty.textContent = "正在读取消息…";
  elements.detailEmpty.hidden = false;
  elements.messageList.replaceChildren();

  const limit = state.info?.settings.message_limit || 300;
  await loadMessages(limit);

  // 记住上次看的会话。
  if (state.info) {
    state.info.settings.last_chat = username;
    invoke("save_settings", { settings: state.info.settings }).catch(() => {});
  }
}

async function loadMessages(limit) {
  try {
    state.messages = await invoke("list_messages", { chat: state.current, limit });
  } catch (error) {
    elements.detailEmpty.textContent = `读取消息失败: ${error}`;
    return;
  }
  renderMessages();
}

async function loadRevoked() {
  elements.more.disabled = true;
  elements.detailEmpty.hidden = false;
  elements.detailEmpty.textContent = "正在逐页比对, 这一步最慢…";
  try {
    const found = await invoke("find_revoked", { chat: state.current });
    const known = new Set(state.messages.filter((item) => item.revoked).map((item) => item.local_id));
    for (const item of found) {
      if (!known.has(item.local_id)) state.messages.push(item);
    }
    state.messages.sort((left, right) => left.create_time - right.create_time);
    renderMessages();
  } catch (error) {
    elements.detailEmpty.textContent = `找回失败: ${error}`;
  } finally {
    elements.more.disabled = false;
  }
}

function renderMessages() {
  const limit = state.info?.settings.message_limit || 300;
  const visible = state.messages.slice(-limit);
  elements.more.hidden = state.messages.length <= limit;
  elements.more.textContent = `显示更多 (共 ${state.messages.length} 条)`;
  elements.detailEmpty.hidden = visible.length > 0;

  elements.messageList.replaceChildren(
    ...visible.map((message) => {
      const item = document.createElement("article");
      item.className = message.revoked ? "message revoked" : "message";

      const head = document.createElement("div");
      head.className = "message-head";

      const sender = document.createElement("span");
      sender.className = "message-sender";
      sender.textContent = message.sender;

      const time = document.createElement("span");
      time.textContent = formatTime(message.create_time);

      const kind = document.createElement("span");
      kind.className = "badge";
      kind.textContent = message.kind;

      head.append(sender, time, kind);
      if (message.revoked) {
        const badge = document.createElement("span");
        badge.className = "badge revoked";
        badge.textContent = "撤回";
        head.append(badge);
      }

      const body = document.createElement("p");
      body.className = message.revoked ? "message-body" : "message-body";
      body.textContent = message.text || "(无正文)";
      body.classList.toggle("dim", !message.text);

      item.append(head, body);
      return item;
    }),
  );
  elements.messageList.scrollTop = elements.messageList.scrollHeight;
}

function bindEvents() {
  elements.chatSearch.addEventListener("input", renderChats);
  elements.revoked.addEventListener("change", async () => {
    state.revokedOnly = elements.revoked.checked;
    if (state.revokedOnly) {
      await loadRevoked();
    } else {
      await loadMessages(state.info?.settings.message_limit || 300);
    }
  });

  elements.more.addEventListener("click", async () => {
    const current = state.info?.settings.message_limit || 300;
    const next = Math.min(current * 2, 5000);
    state.info.settings.message_limit = next;
    elements.settingLimit.value = next;
    await loadMessages(next);
    invoke("save_settings", { settings: state.info.settings }).catch(() => {});
  });

  elements.settingsButton.addEventListener("click", () => elements.settings.showModal());
  elements.settingsSave.addEventListener("click", () => {
    state.info.settings.start_hidden = elements.settingStartHidden.checked;
    state.info.settings.hide_dock_when_hidden = elements.settingHideDock.checked;
    const limit = Number(elements.settingLimit.value) || 300;
    state.info.settings.message_limit = Math.max(20, Math.min(5000, limit));
    invoke("save_settings", { settings: state.info.settings }).catch(() => {});
  });

  elements.quit.addEventListener("click", () => invoke("quit_app"));
}

async function main() {
  bindEvents();
  await loadInfo();
  await loadChats();
}

main().catch((error) => {
  elements.detailEmpty.hidden = false;
  elements.detailEmpty.textContent = `启动失败: ${error}`;
});

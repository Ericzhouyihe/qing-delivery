/** 在线聊天页(007 T053):三栏布局——顶部账号 Tab(在线点+未读徽标 99+)、
 *  左栏会话列表(搜索/只看未读/游标加载更多/删除确认)、右栏消息面板
 *  (气泡:入站左/出站右/系统居中;加载更早 before_id 游标;滚动到底)。
 *  出站四状态渲染(sending/sent/failed 可重试/uncertain 黄警不可自动重发);
 *  输入区(≤2000 字计数、Enter 发送/Shift+Enter 换行、图片能力门禁、粘贴先预览确认);
 *  快捷回复抽屉(≤50 条:复制/发送/删除/新增)、买家备注弹窗(≤2000);
 *  3s 可见性轮询(消息窗口 + unread-summary,发消息后立即刷一次,FR-046)。
 *  "算"在 model.ts;本页只装配与既有 API(宪章 II)。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpGet, httpPost, httpPut, httpRequest } from "../../shared/http";
import { errorHint, isRecord, parseChatCapabilities, type ChatCapabilities } from "../../shared/contracts";
import { btn } from "../../ui/dom";
import { formatAbsolute, formatRelative } from "../../ui/time";
import { startPoll } from "../../ui/poll";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { confirmDialog, openModal } from "../../ui/modal";
import { session } from "../../app/session";
import { setChatUnread } from "../../app/store";
import {
  IMAGE_MAX_BYTES,
  MAX_MESSAGE_CHARS,
  MAX_NOTE_CHARS,
  MAX_QUICK_REPLIES,
  isRenderableImageSrc,
  itemCardView,
  mergeMessages,
  parseConversation,
  parseMessage,
  parseNoteResponse,
  parseQuickReply,
  parseUnreadSummary,
  peerDisplayName,
  quickReplyCapReached,
  sessionsQuery,
  statusView,
  unreadBadgeText,
  validateBuyerNote,
  validateImageFile,
  validateOutgoingText,
  validateQuickReplyBody,
  type ChatMessageDto,
  type ConversationDto,
  type QuickReplyDto,
  type UnreadSummary
} from "./model";
import type { PageFactory } from "../../app/page";

/** 已知错误码集(拼接 errorHint;与 contracts ERROR_HINTS 收录范围一致)。 */
const ERROR_HINTED = new Set(["payload_too_large", "reply_limit_reached"]);

const apiErrorMessage = (e: unknown): string => {
  if (e instanceof ApiHttpError) {
    const code = e.body?.code;
    const hint = code !== undefined && ERROR_HINTED.has(code) ? errorHint(code, e.message) : "";
    return hint.length > 0 ? `${hint}(${e.message})` : e.message;
  }
  return e instanceof Error ? e.message : String(e);
};

/** 侧边栏徽标重算:shell.refresh 只在路由切换触发,聊天页轮询时广播事件补一拍。 */
const notifyBadgesChanged = (): void => {
  document.dispatchEvent(new CustomEvent("qing:badges-changed"));
};

const notice = (message: string): void => {
  confirmDialog({ title: "提示", message, confirmLabel: "知道了" });
};

interface ChatAccount {
  id: string;
  displayName: string;
  connectionState: string;
}

const isOnline = (a: ChatAccount | undefined): boolean => a?.connectionState === "online";
const needsAuth = (a: ChatAccount | undefined): boolean =>
  a?.connectionState === "authorization_expired" || a?.connectionState === "verification_required";

export const chatPage: PageFactory = (root) => {
  // ---- 状态(页面私有;跨页共享仅 chatUnread 徽标通道)----
  let accounts: ChatAccount[] = [];
  let activeAccountId = "";
  let capabilities: ChatCapabilities | null = null; // null = 未加载(图片入口按门禁关闭)
  let unread: UnreadSummary = { total: 0, byAccount: {} };
  let search = "";
  let unreadOnly = false;
  let sessions: ConversationDto[] = [];
  let sessionsCursor: string | null = null;
  let sessionsBusy = false;
  let current: ConversationDto | null = null;
  let messages: ChatMessageDto[] = [];
  let olderCursor: string | null = null; // 消息"加载更早"before_id 游标
  let loadingOlder = false;
  let quickReplies: QuickReplyDto[] = [];
  let drawerOpen = false;
  /** 乐观 sending 占位(POST 在途;响应到达即移除并替换为真实行)。 */
  let pendingOut: Array<{ tempId: string; text: string }> = [];
  let sendSeq = 0;
  let windowSeq = 0;

  const accountOf = (id: string): ChatAccount | undefined => accounts.find((a) => a.id === id);

  // ---- 页头 ----
  const head = el(
    "div",
    { class: "page-head" },
    el("h1", { class: "page-head-title" }, "在线聊天"),
    el(
      "p",
      { class: "page-head-sub" },
      "买家会话与消息(接入后收到的记录);发送状态四态可见,结果未知不自动重发。"
    )
  );

  // ---- 账号 Tab 行(昵称 + 在线点 + 未读徽标 99+)----
  const tabsRow = el("div", { class: "chat-tabs", role: "tablist", "aria-label": "账号会话页签" });
  const tabsHint = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "");

  // ---- 左栏:会话列表 ----
  const searchInput = el("input", {
    class: "search-input",
    type: "search",
    placeholder: "搜索用户 / 商品 / 消息",
    "aria-label": "搜索会话"
  }) as HTMLInputElement;
  const unreadCheck = el("input", { type: "checkbox", role: "switch", "aria-label": "只看未读" }) as HTMLInputElement;
  const filterBar = el(
    "div",
    { class: "chat-session-filter" },
    searchInput,
    el("label", { class: "check-row", style: "white-space:nowrap;font-size:var(--text-xs);" }, unreadCheck, "只看未读")
  );
  const sessionList = el("ul", { class: "chat-session-list" });
  const sessionMoreBtn = el("button", { class: "btn btn-secondary btn-sm", type: "button" }, "加载更多");
  sessionMoreBtn.addEventListener("click", () => void loadSessions(true));
  sessionMoreBtn.hidden = true;
  const sessionSide = el(
    "aside",
    { class: "chat-side", "aria-label": "会话列表" },
    filterBar,
    sessionList,
    el("div", { class: "chat-session-more" }, sessionMoreBtn)
  );

  // ---- 右栏:消息面板 ----
  const panelHead = el("div", { class: "chat-panel-head" });
  const messagesBox = el("div", { class: "chat-messages", role: "log", "aria-label": "消息记录" });
  const inputArea = el("div", { class: "chat-input" });
  const panel = el("section", { class: "chat-panel" }, panelHead, messagesBox, inputArea);

  // ---- 快捷回复抽屉(右 300px)----
  const quickList = el("ul", { class: "chat-quick-list" });
  const quickAddInput = el("input", {
    type: "text",
    placeholder: `新增快捷回复(≤${MAX_MESSAGE_CHARS} 字,回车添加)`,
    "aria-label": "新增快捷回复"
  }) as HTMLInputElement;
  const quickAddBtn = el("button", { class: "btn btn-secondary btn-sm", type: "button" }, "添加");
  const quickCount = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "");
  const quickDrawer = el(
    "aside",
    { class: "chat-quick-drawer", "aria-label": "快捷回复抽屉" },
    el("div", { class: "chat-quick-head" }, el("strong", {}, "快捷回复"), quickCount),
    quickList,
    el("div", { class: "chat-quick-add" }, quickAddInput, quickAddBtn)
  );
  quickDrawer.hidden = true;

  // ---- 三栏布局 ----
  const layout = el("div", { class: "chat-layout" }, sessionSide, panel, quickDrawer);
  root.append(head, el("div", { class: "chat-tabbar" }, tabsRow, tabsHint), layout);

  // ===== 渲染 =====

  const dotClass = (a: ChatAccount | undefined): string =>
    isOnline(a) ? "dot-online" : needsAuth(a) ? "dot-danger" : "dot-offline";

  const renderTabs = (): void => {
    tabsRow.replaceChildren();
    for (const a of accounts) {
      const n = unread.byAccount[a.id] ?? 0;
      const badge = el("span", { class: "chat-badge" }, unreadBadgeText(n));
      badge.hidden = n <= 0;
      const tab = el(
        "button",
        {
          class: `chat-tab${a.id === activeAccountId ? " active" : ""}`,
          type: "button",
          role: "tab",
          "aria-selected": a.id === activeAccountId ? "true" : "false"
        },
        el("span", { class: `chat-dot ${dotClass(a)}`, "aria-hidden": "true" }),
        el("span", { class: "chat-tab-name" }, a.displayName),
        badge
      );
      tab.addEventListener("click", () => {
        if (activeAccountId === a.id) return;
        activeAccountId = a.id;
        current = null;
        messages = [];
        olderCursor = null;
        windowSeq++;
        renderTabs();
        renderPanel();
        void loadSessions(false);
      });
      tabsRow.append(tab);
    }
  };

  const sessionItem = (c: ConversationDto): HTMLElement => {
    const n = c.unreadCount;
    const badge = el("span", { class: "chat-badge" }, unreadBadgeText(n));
    badge.hidden = n <= 0;
    const main = el(
      "button",
      {
        class: `chat-session${current !== null && current.id === c.id ? " active" : ""}`,
        type: "button"
      },
      el("div", { class: "chat-session-row" }, el("strong", { class: "chat-session-name" }, peerDisplayName(c)), badge),
      el("div", { class: "chat-session-id muted" }, `买家 ${c.peerBuyerId}`),
      el(
        "div",
        { class: "chat-session-row" },
        el("span", { class: "chat-session-preview muted" }, c.lastMessagePreview ?? "暂无消息"),
        el(
          "span",
          { class: "chat-session-time muted", title: c.lastMessageAt === null ? "" : formatAbsolute(c.lastMessageAt) },
          c.lastMessageAt === null ? "—" : formatRelative(c.lastMessageAt)
        )
      )
    );
    main.addEventListener("click", () => void openConversation(c));
    const delBtn = el("button", {
      class: "icon-btn danger-icon",
      type: "button",
      title: `删除会话 ${peerDisplayName(c)}(仅本机隐藏)`
    });
    delBtn.textContent = "✕";
    delBtn.addEventListener("click", () => deleteConversation(c));
    return el("li", { class: "chat-session-item" }, main, delBtn);
  };

  const renderSessions = (): void => {
    sessionList.replaceChildren();
    if (sessions.length === 0) {
      const filtering = search.trim().length > 0 || unreadOnly;
      const empty = el("li", { class: "chat-session-empty muted" }, filtering ? "无匹配会话" : "暂无会话");
      if (filtering) {
        const clear = el("button", { class: "btn btn-ghost btn-sm", type: "button" }, "清空筛选");
        clear.addEventListener("click", () => {
          search = "";
          unreadOnly = false;
          searchInput.value = "";
          unreadCheck.checked = false;
          void loadSessions(false);
        });
        empty.append(el("div", { style: "margin-top:6px;" }, clear));
      } else {
        empty.append(
          el("div", { style: "font-size:var(--text-xs);" }, "等待买家来消息;接入后收到的会话在此展示")
        );
        if (capabilities !== null && !capabilities.chatHistoryBackfill) {
          // FR-042 能力如实:未声明历史回填时不虚标"已同步全部历史"
          empty.append(el("div", { class: "chat-caps-note" }, "仅展示接入后收到的会话,不回填历史"));
        }
      }
      sessionList.append(empty);
    } else {
      sessionList.append(...sessions.map(sessionItem));
    }
    sessionMoreBtn.hidden = sessionsCursor === null;
    sessionMoreBtn.disabled = sessionsBusy;
    sessionMoreBtn.textContent = sessionsBusy ? "加载中…" : "加载更多";
  };

  // ---- 消息面板 ----

  const scrollBottom = (): void => {
    messagesBox.scrollTop = messagesBox.scrollHeight;
  };
  const nearBottom = (): boolean =>
    messagesBox.scrollHeight - messagesBox.scrollTop - messagesBox.clientHeight < 80;

  /** 出站状态角标:四状态 + failed 重试按钮;uncertain 重试禁用并说明(零虚标)。 */
  const statusNode = (m: ChatMessageDto): HTMLElement => {
    const wrap = el("span", { class: "chat-msg-status" });
    const view = statusView(m.status);
    if (view !== null) {
      const markTitle =
        m.status === "uncertain" ? `${view.label}:${view.retryDisabledReason}` : view.label;
      wrap.append(
        el("span", { class: `chat-status chat-status-${view.tone}`, title: markTitle }, view.mark),
        el("span", { class: "muted", style: "font-size:var(--text-xs);" }, view.label)
      );
    }
    if (m.errorHint !== null && m.errorHint.length > 0) {
      wrap.append(el("span", { class: "chat-status-hint" }, m.errorHint));
    }
    if (view !== null && view.canRetry) {
      const retry = el("button", { class: "btn btn-secondary btn-sm", type: "button" }, "重试");
      retry.addEventListener("click", () => void retryMessage(m.id, retry));
      wrap.append(retry);
    } else if (view !== null && m.status !== "sent") {
      const blockedRetry = el("button", { class: "btn btn-secondary btn-sm", type: "button" }, "重试");
      blockedRetry.disabled = true;
      blockedRetry.title = view.retryDisabledReason;
      wrap.append(blockedRetry);
    }
    return wrap;
  };

  /** 气泡内容:文本/图片(可直连才 <img> 懒加载)/系统/商品卡片/未知占位。 */
  const bubbleBody = (m: ChatMessageDto): HTMLElement => {
    const body = el("div", { class: "chat-bubble-body" });
    if (m.kind === "image") {
      if (isRenderableImageSrc(m.imagePath)) {
        body.append(el("img", { src: m.imagePath ?? "", alt: "图片消息", loading: "lazy", class: "chat-img" }));
      } else {
        // 本地数据目录路径(uploads/chat/…)无 HTTP 静态路由:如实占位,不渲染裂图
        body.append(el("span", { class: "chat-img-fallback", title: m.imagePath ?? "" }, "[图片]"));
      }
      if (m.bodyText !== null && m.bodyText.length > 0) body.append(el("span", {}, m.bodyText));
      return body;
    }
    if (m.kind === "item_card") {
      const card = itemCardView(m.itemSnapshot);
      if (card === null) {
        body.append(el("span", { class: "muted" }, "[商品卡片]"));
      } else {
        const box = el("div", { class: "chat-item-card" });
        if (card.imageUrl !== null) {
          box.append(el("img", { src: card.imageUrl, alt: card.title, loading: "lazy", class: "chat-item-pic" }));
        }
        box.append(el("div", { class: "chat-item-title" }, card.title));
        if (card.price !== null) box.append(el("div", { class: "chat-item-price" }, card.price));
        body.append(box);
      }
      if (m.bodyText !== null && m.bodyText.length > 0) body.append(el("div", {}, m.bodyText));
      return body;
    }
    if (m.kind === "system") {
      body.append(el("span", {}, m.bodyText ?? "[系统消息]"));
      return body;
    }
    body.append(el("span", { class: "chat-text" }, m.bodyText ?? (m.kind === "unknown" ? "[消息]" : "")));
    return body;
  };

  const messageNode = (m: ChatMessageDto): HTMLElement => {
    if (m.kind === "system") {
      // 系统通知:居中灰气泡,不占左右对话位
      return el(
        "li",
        { class: "chat-row chat-row-system" },
        el("div", { class: "chat-bubble chat-bubble-system", title: formatAbsolute(m.createdAt) }, bubbleBody(m))
      );
    }
    const outgoing = m.direction === "out";
    return el(
      "li",
      { class: `chat-row ${outgoing ? "chat-row-out" : "chat-row-in"}` },
      el(
        "div",
        { class: `chat-bubble ${outgoing ? "chat-bubble-out" : "chat-bubble-in"}` },
        bubbleBody(m),
        el(
          "div",
          { class: "chat-bubble-meta" },
          el("span", { class: "muted", title: formatAbsolute(m.createdAt) }, formatRelative(m.createdAt)),
          outgoing ? statusNode(m) : null
        )
      )
    );
  };

  const pendingNode = (p: { text: string }): HTMLElement =>
    el(
      "li",
      { class: "chat-row chat-row-out" },
      el(
        "div",
        { class: "chat-bubble chat-bubble-out" },
        el("div", { class: "chat-bubble-body" }, el("span", { class: "chat-text" }, p.text)),
        el(
          "div",
          { class: "chat-bubble-meta" },
          el("span", { class: "chat-status chat-status-neutral", "aria-hidden": "true" }, "◷"),
          el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "发送中…")
        )
      )
    );

  const renderMessages = (opts: { keepScroll: boolean }): void => {
    const prevTop = messagesBox.scrollTop;
    const prevHeight = messagesBox.scrollHeight;
    const wasBottom = nearBottom();
    messagesBox.replaceChildren();
    if (olderCursor !== null) {
      const earlier = el("button", { class: "btn btn-ghost btn-sm", type: "button" }, "加载更早消息");
      earlier.disabled = loadingOlder;
      if (loadingOlder) earlier.textContent = "加载中…";
      earlier.addEventListener("click", () => void loadOlder());
      messagesBox.append(el("div", { class: "chat-load-earlier-row" }, earlier));
    }
    for (const m of messages) messagesBox.append(messageNode(m));
    for (const p of pendingOut) messagesBox.append(pendingNode(p));
    if (opts.keepScroll) {
      if (wasBottom) scrollBottom();
      else messagesBox.scrollTop = prevTop + (messagesBox.scrollHeight - prevHeight);
    }
  };

  // ---- 面板头 + 输入区 ----

  const renderPanel = (): void => {
    panelHead.replaceChildren();
    inputArea.replaceChildren();
    if (current === null) {
      panelHead.append(el("span", { class: "muted" }, "未选择会话"));
      messagesBox.replaceChildren(
        el(
          "div",
          { class: "empty-state chat-panel-empty" },
          el("div", { class: "empty-ico" }, "✉"),
          el("div", {}, "选择左侧会话开始聊天"),
          el("div", { class: "muted" }, "会话按账号分组;打开会话后未读清零")
        )
      );
      return;
    }
    const acct = accountOf(current.accountId);
    const stateText = isOnline(acct) ? "在线" : (acct?.connectionState ?? "离线");
    const noteBtn = el("button", { class: "btn btn-secondary btn-sm", type: "button" }, "备注");
    noteBtn.title = "查看/编辑买家备注(仅卖家视角)";
    noteBtn.addEventListener("click", () => openBuyerNote(noteBtn));
    panelHead.append(
      el(
        "div",
        { class: "chat-panel-title" },
        el("strong", {}, peerDisplayName(current)),
        el("span", { class: "muted", style: "font-size:var(--text-xs);" }, `买家 ${current.peerBuyerId}`),
        el(
          "span",
          { class: `chat-acct-state ${isOnline(acct) ? "is-online" : "is-offline"}` },
          el("span", { class: `chat-dot ${dotClass(acct)}`, "aria-hidden": "true" }),
          stateText
        )
      ),
      el("div", { class: "chat-panel-actions" }, noteBtn)
    );
    buildInputArea();
    renderMessages({ keepScroll: false });
    scrollBottom();
  };

  const buildInputArea = (): void => {
    if (current === null) return;
    const conv = current;
    const online = isOnline(accountOf(conv.accountId));
    const imageSupported = capabilities?.chatSendImage === true;

    const textarea = el("textarea", {
      class: "chat-textarea",
      rows: "3",
      placeholder: online ? "输入消息,Enter 发送 / Shift+Enter 换行;可粘贴图片" : "账号离线,暂不能发送消息",
      "aria-label": "消息输入框"
    }) as HTMLTextAreaElement;
    textarea.disabled = !online;
    const counter = el("span", { class: "chat-counter muted" }, `0/${MAX_MESSAGE_CHARS}`);
    const errLine = el("div", { class: "field-error", role: "alert" }, "");
    const sendBtn = btn("发送", { variant: "primary" });
    const imageBtn = el("button", { class: "btn btn-secondary btn-sm", type: "button" }, "🖼 图片");
    const fileInput = el("input", { type: "file", accept: "image/png,image/jpeg,image/gif,image/webp" }) as HTMLInputElement;
    fileInput.hidden = true;
    const drawerBtn = el("button", { class: "btn btn-ghost btn-sm", type: "button" }, "☰ 快捷回复");

    const syncCounter = (): void => {
      const n = [...textarea.value].length;
      counter.textContent = `${n}/${MAX_MESSAGE_CHARS}`;
      counter.classList.toggle("chat-counter-over", n > MAX_MESSAGE_CHARS);
    };
    const syncSend = (): void => {
      const v = validateOutgoingText(textarea.value);
      sendBtn.disabled = !online || !v.ok || pendingOut.length > 0;
      sendBtn.title = online ? (v.ok ? "" : v.error) : "账号离线,暂不能发送消息";
    };
    textarea.addEventListener("input", () => {
      syncCounter();
      syncSend();
      errLine.textContent = "";
    });
    textarea.addEventListener("keydown", (ev) => {
      if (ev.key !== "Enter" || ev.shiftKey || ev.isComposing) return;
      ev.preventDefault();
      void doSend();
    });
    // 粘贴图片:先预览确认(FR-041);纯文本粘贴不受影响
    textarea.addEventListener("paste", (ev) => {
      const file = [...(ev.clipboardData?.files ?? [])].find((f) => f.type.startsWith("image/"));
      if (file === undefined) return;
      ev.preventDefault();
      previewAndSendImage(file);
    });

    const doSend = async (): Promise<void> => {
      const text = textarea.value;
      const v = validateOutgoingText(text);
      if (!v.ok) {
        errLine.textContent = v.error;
        return;
      }
      const ok = await sendText(text);
      if (ok) {
        textarea.value = "";
        syncCounter();
        syncSend();
        textarea.focus();
      }
    };
    sendBtn.addEventListener("click", () => void doSend());

    // 图片能力门禁(FR-042):未声明/未知 → 禁用并如实说明,不虚报可用
    if (!online) {
      imageBtn.disabled = true;
      imageBtn.title = "账号离线,暂不能发送消息";
    } else if (!imageSupported) {
      imageBtn.disabled = true;
      imageBtn.title =
        capabilities === null
          ? "能力信息不可用,图片发送暂不可用"
          : "当前适配器未声明图片发送能力(chat_send_image=false)";
    }
    imageBtn.addEventListener("click", () => fileInput.click());
    fileInput.addEventListener("change", () => {
      const f = fileInput.files?.[0];
      fileInput.value = "";
      if (f !== undefined) previewAndSendImage(f);
    });

    drawerBtn.addEventListener("click", () => {
      drawerOpen = !drawerOpen;
      quickDrawer.hidden = !drawerOpen;
      drawerBtn.classList.toggle("active", drawerOpen);
    });
    drawerBtn.classList.toggle("active", drawerOpen);

    inputArea.append(
      el(
        "div",
        { class: "chat-input-row" },
        imageBtn,
        textarea,
        el("div", { class: "chat-input-actions" }, sendBtn, drawerBtn)
      ),
      el(
        "div",
        { class: "chat-input-foot" },
        counter,
        el(
          "span",
          { class: "muted", style: "font-size:var(--text-xs);" },
          online ? "Enter 发送 · Shift+Enter 换行" : "账号离线,暂不能发送消息"
        ),
        fileInput
      ),
      errLine
    );
    syncCounter();
    syncSend();
  };

  // ===== 快捷回复抽屉 =====

  const renderQuickReplies = (): void => {
    quickCount.textContent = `${quickReplies.length}/${MAX_QUICK_REPLIES}`;
    const capped = quickReplyCapReached(quickReplies.length);
    quickAddInput.disabled = capped;
    quickAddBtn.disabled = capped;
    const capHint = capped ? `已达上限 ${MAX_QUICK_REPLIES} 条,删除后可再新增` : "";
    quickAddInput.title = capHint;
    quickAddBtn.title = capHint;
    quickList.replaceChildren();
    if (quickReplies.length === 0) {
      quickList.append(
        el("li", { class: "muted", style: "font-size:var(--text-xs);" }, "暂无快捷回复;在下方输入后回车添加")
      );
      return;
    }
    for (const q of quickReplies) {
      const copyBtn = el("button", { class: "btn btn-ghost btn-sm", type: "button", title: "复制到输入框" }, "复制");
      const sendQuickBtn = el("button", {
        class: "btn btn-secondary btn-sm",
        type: "button",
        title: "直接发送给当前会话"
      }, "发送");
      const delBtn = el("button", { class: "icon-btn danger-icon", type: "button", title: "删除该快捷回复" }, "✕");
      copyBtn.addEventListener("click", () => {
        const ta = inputArea.querySelector<HTMLTextAreaElement>(".chat-textarea");
        if (ta === null || ta.disabled) {
          // 未开会话/账号离线:复制到新增框暂存,避免静默丢弃
          quickAddInput.value = q.body;
          quickAddInput.focus();
          return;
        }
        ta.value = q.body;
        ta.dispatchEvent(new Event("input", { bubbles: true }));
        ta.focus();
      });
      sendQuickBtn.addEventListener("click", () => {
        if (current === null) {
          notice("请先在左侧选择会话,再直接发送快捷回复");
          return;
        }
        void sendText(q.body);
      });
      delBtn.addEventListener("click", () => {
        openConfirmFlow({
          title: "删除快捷回复",
          impact: `将删除快捷回复「${q.body.slice(0, 24)}${q.body.length > 24 ? "…" : ""}」。此操作仅影响快捷回复列表。`,
          requireReason: false,
          requireRiskAck: false,
          danger: true,
          confirmLabel: "删除",
          submit: async () => {
            try {
              await httpRequest<void>(`/api/v1/chat/quick-replies/${q.id}`, { method: "DELETE" });
            } catch (e) {
              throw new Error(apiErrorMessage(e));
            }
          },
          onSuccess: () => void loadQuickReplies()
        });
      });
      quickList.append(
        el(
          "li",
          { class: "chat-quick-item" },
          el("div", { class: "chat-quick-body", title: q.body }, q.body),
          el("div", { class: "chat-quick-actions" }, copyBtn, sendQuickBtn, delBtn)
        )
      );
    }
  };

  const addQuickReply = async (): Promise<void> => {
    const body = quickAddInput.value;
    const v = validateQuickReplyBody(body);
    if (!v.ok) {
      notice(v.error);
      return;
    }
    if (quickReplyCapReached(quickReplies.length)) {
      notice(`快捷回复已达上限 ${MAX_QUICK_REPLIES} 条,删除后可再新增`);
      return;
    }
    try {
      await httpPost("/api/v1/chat/quick-replies", { body });
      quickAddInput.value = "";
      await loadQuickReplies();
    } catch (e) {
      notice(`新增失败:${apiErrorMessage(e)}`);
    }
  };
  quickAddBtn.addEventListener("click", () => void addQuickReply());
  quickAddInput.addEventListener("keydown", (ev) => {
    if (ev.key !== "Enter" || ev.isComposing) return;
    ev.preventDefault();
    void addQuickReply();
  });

  // ===== 买家备注弹窗(后端会话列表即卖家视角,直接展示入口)=====

  const openBuyerNote = (opener: HTMLElement): void => {
    if (current === null) return;
    const conv = current;
    const handle = openModal(opener, { title: `买家备注 · ${peerDisplayName(conv)}` });
    const textarea = el("textarea", {
      class: "chat-note-textarea",
      rows: "6",
      placeholder: "记录该买家的备注(≤2000 字,保存后仅本机可见)"
    }) as HTMLTextAreaElement;
    const counter = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, `0/${MAX_NOTE_CHARS}`);
    const errLine = el("div", { class: "field-error" }, "");
    const status = el("div", { class: "muted", style: "font-size:var(--text-xs);" }, "加载中…");
    let dirty = false;
    let saving = false;
    textarea.addEventListener("input", () => {
      dirty = true;
      const n = [...textarea.value].length;
      counter.textContent = `${n}/${MAX_NOTE_CHARS}`;
      counter.classList.toggle("chat-counter-over", n > MAX_NOTE_CHARS);
      errLine.textContent = "";
    });
    const saveBtn = btn("保存", { variant: "primary" });
    const closeBtn = btn("关闭", { variant: "secondary" });
    saveBtn.addEventListener("click", async () => {
      const v = validateBuyerNote(textarea.value);
      if (!v.ok) {
        errLine.textContent = v.error;
        return;
      }
      saveBtn.disabled = true;
      saving = true;
      try {
        await httpPut(`/api/v1/chat/accounts/${conv.accountId}/buyer-notes/${encodeURIComponent(conv.peerBuyerId)}`, {
          note: textarea.value
        });
        dirty = false;
        handle.close(true);
      } catch (e) {
        errLine.textContent = `保存失败:${apiErrorMessage(e)}`;
        saveBtn.disabled = false;
      } finally {
        saving = false;
      }
    });
    closeBtn.addEventListener("click", () => {
      if (dirty && !saving && !window.confirm("备注未保存,确定关闭?")) return;
      handle.close(true);
    });
    handle.body.append(
      el("div", { class: "field" }, el("label", {}, `买家 ${conv.peerBuyerId} 的备注`), textarea, counter),
      status,
      errLine,
      el("div", { style: "display:flex;gap:8px;justify-content:flex-end;margin-top:12px;" }, closeBtn, saveBtn)
    );
    void (async () => {
      try {
        const note = parseNoteResponse(
          await httpGet(`/api/v1/chat/accounts/${conv.accountId}/buyer-notes/${encodeURIComponent(conv.peerBuyerId)}`)
        );
        textarea.value = note;
        counter.textContent = `${[...note].length}/${MAX_NOTE_CHARS}`;
        status.textContent = note.length === 0 ? "暂无备注" : "";
      } catch (e) {
        status.textContent = `备注加载失败:${apiErrorMessage(e)}`;
      }
    })();
  };

  // ===== 数据装载 =====

  const loadAccounts = async (): Promise<void> => {
    const raw = await httpGet("/api/v1/accounts");
    const items = (isRecord(raw) && Array.isArray(raw["items"]) ? raw["items"] : []) as unknown[];
    accounts = items.map((a): ChatAccount => {
      const r = isRecord(a) ? a : {};
      return {
        id: String(r["id"] ?? ""),
        displayName: String(r["display_name"] ?? r["id"] ?? ""),
        connectionState: String(r["connection_state"] ?? "paused")
      };
    });
    if (activeAccountId === "" || !accounts.some((a) => a.id === activeAccountId)) {
      activeAccountId = accounts[0]?.id ?? "";
    }
    renderTabs();
  };

  const loadCapabilities = async (): Promise<void> => {
    try {
      capabilities = parseChatCapabilities(await httpGet("/api/v1/capabilities"));
    } catch {
      capabilities = { chatSendImage: false, chatHistoryBackfill: false }; // 不可用按关闭门禁(零虚标)
    }
    renderSessions(); // 能力说明可能改变空态文案
  };

  const loadUnread = async (): Promise<void> => {
    unread = parseUnreadSummary(await httpGet("/api/v1/chat/unread-summary"));
    setChatUnread(unread.total); // 侧边栏徽标(store 通道;路由切换时 getter 重算)
    notifyBadgesChanged(); // 聊天页在场时立即重绘侧边栏徽标
    renderTabs();
  };

  const loadSessions = async (append: boolean): Promise<void> => {
    if (activeAccountId === "" || sessionsBusy) return;
    sessionsBusy = true;
    renderSessions();
    try {
      const cursor = append ? sessionsCursor : null;
      const raw = await httpGet(
        `/api/v1/accounts/${activeAccountId}/chat/sessions?${sessionsQuery(search, unreadOnly, cursor)}`
      );
      const page = (isRecord(raw) ? raw : {}) as Record<string, unknown>;
      const items = Array.isArray(page["items"]) ? page["items"] : [];
      const parsed = items.map(parseConversation);
      sessions = append ? [...sessions, ...parsed] : parsed;
      sessionsCursor = typeof page["next_cursor"] === "string" ? page["next_cursor"] : null;
    } catch {
      if (!append) sessions = [];
      sessionsCursor = null;
      sessionList.replaceChildren(
        el("li", { class: "chat-session-empty error-text" }, "会话加载失败,请稍后重试")
      );
      sessionMoreBtn.hidden = true;
      sessionsBusy = false;
      return;
    }
    sessionsBusy = false;
    renderSessions();
  };

  /** 打开会话:载消息 + 未读清零(FR-043)+ 会话列表同步。 */
  const openConversation = async (c: ConversationDto): Promise<void> => {
    if (current !== null && current.id === c.id) return;
    current = c;
    messages = [];
    olderCursor = null;
    pendingOut = [];
    windowSeq++;
    renderSessions();
    renderPanel();
    try {
      await refreshWindow(false);
    } catch {
      messagesBox.replaceChildren(
        el("div", { class: "error-block", role: "alert" }, "消息加载失败;等待下次自动刷新或切换会话重试")
      );
    }
    if (current === null || current.id !== c.id) return; // 期间已切换/删除
    const dec = c.unreadCount;
    try {
      await httpPost(`/api/v1/chat/conversations/${c.id}/read`);
    } catch {
      return; // 未读清零失败不阻塞阅读;下轮轮询的 unread-summary 会校正
    }
    if (current === null || current.id !== c.id) return;
    if (dec > 0) {
      current = { ...current, unreadCount: 0 };
      sessions = sessions.map((s) => (s.id === c.id ? { ...s, unreadCount: 0 } : s));
      const left = Math.max(0, (unread.byAccount[c.accountId] ?? 0) - dec);
      const byAccount = { ...unread.byAccount, [c.accountId]: left };
      if (left <= 0) delete byAccount[c.accountId];
      unread = { total: Math.max(0, unread.total - dec), byAccount };
      setChatUnread(unread.total);
      notifyBadgesChanged();
      renderTabs();
      renderSessions();
    }
  };

  /** 拉取最新消息窗口(新→旧 ≤50)并合并进展示(保留已翻出的更早页)。 */
  const refreshWindow = async (keepScroll: boolean): Promise<void> => {
    if (current === null) return;
    const convId = current.id;
    const seq = ++windowSeq;
    const raw = await httpGet(`/api/v1/chat/conversations/${convId}/messages?limit=50`);
    if (seq !== windowSeq || current === null || current.id !== convId) return;
    const page = (isRecord(raw) ? raw : {}) as Record<string, unknown>;
    const items = Array.isArray(page["items"]) ? page["items"] : [];
    messages = mergeMessages(messages, items.map(parseMessage));
    // next_cursor=null 即已到最早;不再臆造"还有更早"
    olderCursor = typeof page["next_cursor"] === "string" ? page["next_cursor"] : null;
    renderMessages({ keepScroll });
  };

  const loadOlder = async (): Promise<void> => {
    if (current === null || olderCursor === null || loadingOlder) return;
    loadingOlder = true;
    renderMessages({ keepScroll: true });
    try {
      const raw = await httpGet(
        `/api/v1/chat/conversations/${current.id}/messages?before_id=${encodeURIComponent(olderCursor)}&limit=50`
      );
      const page = (isRecord(raw) ? raw : {}) as Record<string, unknown>;
      const items = Array.isArray(page["items"]) ? page["items"] : [];
      messages = mergeMessages(messages, items.map(parseMessage));
      olderCursor = typeof page["next_cursor"] === "string" ? page["next_cursor"] : null;
    } catch {
      // 失败保持现状可再点;补历史不阻塞最新窗口
    }
    loadingOlder = false;
    renderMessages({ keepScroll: true });
  };

  /** 发送文本:乐观 sending 占位 → 响应(同步等待终态)替换;失败如实提示。 */
  const sendText = async (text: string): Promise<boolean> => {
    if (current === null) {
      notice("请先在左侧选择会话");
      return false;
    }
    const conv = current;
    const v = validateOutgoingText(text);
    if (!v.ok) {
      notice(v.error);
      return false;
    }
    if (!isOnline(accountOf(conv.accountId))) {
      notice("账号离线,暂不能发送消息");
      return false;
    }
    const tempId = `pending-${++sendSeq}`;
    pendingOut = [...pendingOut, { tempId, text }];
    renderMessages({ keepScroll: false });
    scrollBottom();
    try {
      const row = parseMessage(await httpPost(`/api/v1/chat/conversations/${conv.id}/messages`, { text }));
      pendingOut = pendingOut.filter((p) => p.tempId !== tempId);
      messages = mergeMessages(messages, [row]);
      renderMessages({ keepScroll: false });
      scrollBottom();
      // 发消息后立即刷一次(会话摘要/未读)
      void loadUnread()
        .then(() => loadSessions(false))
        .catch(() => undefined);
      return true;
    } catch (e) {
      pendingOut = pendingOut.filter((p) => p.tempId !== tempId);
      renderMessages({ keepScroll: true });
      const line = inputArea.querySelector(".field-error");
      if (line !== null) line.textContent = `发送失败:${apiErrorMessage(e)}`;
      else notice(`发送失败:${apiErrorMessage(e)}`);
      return false;
    }
  };

  /** 图片发送:预览确认 → multipart 上传(≤10MB;能力门禁 FR-042)。 */
  const previewAndSendImage = (file: File): void => {
    if (current === null) {
      notice("请先在左侧选择会话");
      return;
    }
    const conv = current;
    const check = validateImageFile(file.name, file.size);
    if (!check.ok) {
      notice(check.error);
      return;
    }
    if (capabilities?.chatSendImage !== true) {
      notice(
        capabilities === null
          ? "能力信息不可用,图片发送暂不可用"
          : "当前适配器未声明图片发送能力(chat_send_image=false)"
      );
      return;
    }
    if (!isOnline(accountOf(conv.accountId))) {
      notice("账号离线,暂不能发送消息");
      return;
    }
    const handle = openModal(null, { title: "发送图片(先预览确认)" });
    const url = URL.createObjectURL(file);
    const errLine = el("div", { class: "field-error" }, "");
    const sendBtn = btn("确认发送", { variant: "primary" });
    const cancelBtn = btn("取消", { variant: "secondary" });
    const cleanup = (): void => URL.revokeObjectURL(url);
    sendBtn.addEventListener("click", async () => {
      sendBtn.disabled = true;
      sendBtn.textContent = "发送中…";
      try {
        const row = parseMessage(await uploadImage(conv.id, file));
        cleanup();
        handle.close(true);
        messages = mergeMessages(messages, [row]);
        renderMessages({ keepScroll: false });
        scrollBottom();
        void loadUnread()
          .then(() => loadSessions(false))
          .catch(() => undefined);
      } catch (e) {
        errLine.textContent = `发送失败:${apiErrorMessage(e)}`;
        sendBtn.disabled = false;
        sendBtn.textContent = "确认发送";
      }
    });
    cancelBtn.addEventListener("click", () => {
      cleanup();
      handle.close(true);
    });
    handle.body.append(
      el("img", { src: url, alt: "图片预览", class: "chat-img-preview" }),
      el("div", { class: "muted", style: "font-size:var(--text-xs);" }, `${file.name}(${Math.round(file.size / 1024)} KB)`),
      errLine,
      el("div", { style: "display:flex;gap:8px;justify-content:flex-end;margin-top:12px;" }, cancelBtn, sendBtn)
    );
  };

  /** multipart 上传(http.ts 只发 JSON;与 cards 批量导入同约定:CSRF+幂等键+错误信封)。 */
  const uploadImage = async (conversationId: string, file: File): Promise<unknown> => {
    if (file.size > IMAGE_MAX_BYTES) {
      throw new Error("图片超过 10MB 上限");
    }
    const fd = new FormData();
    fd.append("file", file, file.name);
    const res = await fetch(`/api/v1/chat/conversations/${conversationId}/images`, {
      method: "POST",
      headers: { "X-CSRF-Token": session.csrfToken, "X-Idempotency-Key": crypto.randomUUID() },
      body: fd,
      credentials: "same-origin"
    });
    if ((res.status === 401 || res.status === 403) && session.authenticated) {
      document.dispatchEvent(new CustomEvent("qing:session-expired"));
    }
    const text = await res.text();
    const payload: unknown = text.length > 0 ? JSON.parse(text) : null;
    if (!res.ok) {
      const body = payload as Record<string, unknown> | null;
      const errBody = (body?.["error"] as ApiHttpError["body"] | undefined) ?? null;
      throw new ApiHttpError(res.status, errBody);
    }
    return payload;
  };

  /** 仅 failed 可人工重试(新 attempt=新行);uncertain 由后端 422 unsafe_retry 拒绝。 */
  const retryMessage = async (messageId: string, opener: HTMLButtonElement): Promise<void> => {
    if (current === null) return;
    opener.disabled = true;
    opener.textContent = "重试中…";
    try {
      const row = parseMessage(await httpPost(`/api/v1/chat/messages/${messageId}/retry`));
      messages = mergeMessages(messages, [row]);
      renderMessages({ keepScroll: true });
      void loadSessions(false).catch(() => undefined);
    } catch (e) {
      opener.disabled = false;
      opener.textContent = "重试";
      notice(`重试失败:${apiErrorMessage(e)}`);
    }
  };

  /** 删除会话:确认流;仅本机隐藏+清空展示消息(FR-045,不影响平台侧)。 */
  const deleteConversation = (c: ConversationDto): void => {
    openConfirmFlow({
      title: "删除会话",
      impact: el(
        "div",
        {},
        el("p", { class: "error-text", style: "margin:0 0 6px;font-weight:600;" }, "此操作在本机隐藏会话并清空展示消息"),
        el(
          "p",
          { class: "muted", style: "margin:0;" },
          `会话「${peerDisplayName(c)}」将不再展示;平台侧聊天记录不受影响,买家再次来消息会重新出现。`
        )
      ),
      requireReason: false,
      requireRiskAck: false,
      danger: true,
      confirmLabel: "删除会话",
      submit: async () => {
        try {
          await httpRequest<void>(`/api/v1/chat/conversations/${c.id}`, { method: "DELETE" });
        } catch (e) {
          throw new Error(apiErrorMessage(e));
        }
      },
      onSuccess: () => {
        sessions = sessions.filter((s) => s.id !== c.id);
        if (current !== null && current.id === c.id) {
          current = null;
          messages = [];
          olderCursor = null;
          pendingOut = [];
          windowSeq++;
          renderPanel();
        }
        renderSessions();
        void loadUnread().catch(() => undefined);
      }
    });
  };

  const loadQuickReplies = async (): Promise<void> => {
    try {
      const raw = await httpGet("/api/v1/chat/quick-replies");
      const page = (isRecord(raw) ? raw : {}) as Record<string, unknown>;
      const items = Array.isArray(page["items"]) ? page["items"] : [];
      quickReplies = items.map(parseQuickReply);
    } catch {
      quickReplies = []; // 抽屉是辅助入口:失败降级为空,不阻塞聊天主流程
    }
    renderQuickReplies();
  };

  // ---- 搜索(去抖 300ms)与只看未读(服务端过滤,刷新首页)----
  let searchTimer: number | undefined;
  searchInput.addEventListener("input", () => {
    window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => {
      search = searchInput.value;
      void loadSessions(false);
    }, 300);
  });
  unreadCheck.addEventListener("change", () => {
    unreadOnly = unreadCheck.checked;
    void loadSessions(false);
  });

  // ===== 轮询(FR-046:可见时 3s;不可见自动暂停由 startPoll 内建)=====

  const poll = startPoll(async () => {
    const prevTotal = unread.total;
    const prevMap = JSON.stringify(unread.byAccount);
    try {
      await loadUnread();
    } catch {
      return; // 本轮失败:下轮再试,不打断页面
    }
    const unreadChanged = unread.total !== prevTotal || JSON.stringify(unread.byAccount) !== prevMap;
    if (current !== null) {
      try {
        await refreshWindow(true);
      } catch {
        // 消息窗口失败:保留已展示内容
      }
      if (unreadChanged) void loadSessions(false).catch(() => undefined);
    }
  }, 3000);

  // ===== 初始化 =====
  const boot = async (): Promise<void> => {
    try {
      await loadAccounts();
    } catch (e) {
      tabsHint.textContent = `账号加载失败:${apiErrorMessage(e)}`;
      tabsHint.className = "error-text";
      return;
    }
    if (accounts.length === 0) {
      tabsHint.textContent = "暂无账号;请先在「账号」页接入";
      renderSessions();
      return;
    }
    tabsHint.textContent = "每 3 秒自动刷新(页面可见时);未读徽标 99+ 封顶";
    renderPanel();
    await Promise.allSettled([loadCapabilities(), loadUnread(), loadQuickReplies()]);
    await loadSessions(false);
  };
  void boot();

  return () => {
    poll.stop();
    window.clearTimeout(searchTimer);
  };
};

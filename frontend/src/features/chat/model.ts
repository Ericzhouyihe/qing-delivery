/** 在线聊天视图模型(007 T053/T054):契约解析(MessageDto 四状态/五类型降级、
 *  ConversationDto、unread-summary、快捷回复、买家备注)、输入校验(2000/空)、
 *  出站状态视图、未读徽标与消息合并等纯函数;"算"与"摆"分离(宪章 II),
 *  业务规则以后端 src/domain/chat.rs 为准,前端预校验仅做即时反馈。 */
import { isRecord } from "../../shared/contracts";

// ===== 边界常量(与 src/domain/chat.rs 同值)=====

/** 单条消息正文的字符上限(unicode 标量,FR-041)。 */
export const MAX_MESSAGE_CHARS = 2000;
/** 快捷回复条数上限(FR-044)。 */
export const MAX_QUICK_REPLIES = 50;
/** 买家备注字符上限。 */
export const MAX_NOTE_CHARS = 2000;
/** 图片大小上限(与后端 IMAGE_MAX_BYTES 同值)。 */
export const IMAGE_MAX_BYTES = 10 * 1024 * 1024;
/** 图片扩展名白名单(与后端 IMAGE_EXTS 同值)。 */
export const IMAGE_EXTS: readonly string[] = ["jpg", "jpeg", "png", "gif", "webp"];

// ===== 类型词汇表(domain::chat 枚举的字符串值)=====

export type ChatDirection = "in" | "out";
export type ChatMsgKind = "text" | "image" | "system" | "item_card" | "unknown";
/** 出站四状态(域内另有 cancelled;uncertain 永不自动重发)。 */
export type OutgoingStatus = "sending" | "sent" | "failed" | "uncertain" | "cancelled";

const KINDS: ReadonlySet<string> = new Set(["text", "image", "system", "item_card", "unknown"]);
const STATUSES: ReadonlySet<string> = new Set(["sending", "sent", "failed", "uncertain", "cancelled"]);

// ===== 契约解析(transport chat_api.rs DTO;只校验消费字段,缺失降级)=====

export interface ChatMessageDto {
  id: string;
  conversationId: string;
  direction: ChatDirection;
  kind: ChatMsgKind;
  bodyText: string | null;
  imagePath: string | null;
  /** 仅出站行有状态;入站/未知值降级为 null(不冒充状态)。 */
  status: OutgoingStatus | null;
  errorHint: string | null;
  /** 商品卡片快照(item_card 消息;任意 JSON,缺失为 null)。 */
  itemSnapshot: Record<string, unknown> | null;
  createdAt: string;
  readAt: string | null;
}

function optString(v: unknown): string | null {
  return typeof v === "string" && v.length > 0 ? v : null;
}

export function parseMessage(v: unknown): ChatMessageDto {
  if (!isRecord(v)) throw new Error("消息格式错误");
  const kindRaw = String(v["msg_kind"] ?? "");
  const statusRaw = optString(v["status"]);
  const snapshot = isRecord(v["item_snapshot"]) ? (v["item_snapshot"] as Record<string, unknown>) : null;
  return {
    id: String(v["id"] ?? ""),
    conversationId: String(v["conversation_id"] ?? ""),
    // 非法方向降级为 in(入站渲染);out 是强语义(出站右列+状态),不臆造
    direction: v["direction"] === "out" ? "out" : "in",
    kind: KINDS.has(kindRaw) ? (kindRaw as ChatMsgKind) : "unknown",
    bodyText: optString(v["body_text"]),
    imagePath: optString(v["image_path"]),
    status: statusRaw !== null && STATUSES.has(statusRaw) ? (statusRaw as OutgoingStatus) : null,
    errorHint: optString(v["error_hint"]),
    itemSnapshot: snapshot,
    createdAt: String(v["created_at"] ?? ""),
    readAt: optString(v["read_at"])
  };
}

export interface ConversationDto {
  id: string;
  accountId: string;
  peerBuyerId: string;
  peerNickname: string | null;
  chatId: string | null;
  lastItemId: string | null;
  lastMessageAt: string | null;
  lastMessagePreview: string | null;
  unreadCount: number;
}

export function parseConversation(v: unknown): ConversationDto {
  if (!isRecord(v)) throw new Error("会话格式错误");
  return {
    id: String(v["id"] ?? ""),
    accountId: String(v["account_id"] ?? ""),
    peerBuyerId: String(v["peer_buyer_id"] ?? ""),
    peerNickname: optString(v["peer_nickname"]),
    chatId: optString(v["chat_id"]),
    lastItemId: optString(v["last_item_id"]),
    lastMessageAt: optString(v["last_message_at"]),
    lastMessagePreview: optString(v["last_message_preview"]),
    unreadCount: typeof v["unread_count"] === "number" ? v["unread_count"] : 0
  };
}

export interface UnreadSummary {
  total: number;
  byAccount: Record<string, number>;
}

/** GET /chat/unread-summary:total 缺失时按 by_account 求和派生(不冒充 0)。 */
export function parseUnreadSummary(v: unknown): UnreadSummary {
  const rec = isRecord(v) ? v : {};
  const byAccount: Record<string, number> = {};
  const raw = isRecord(rec["by_account"]) ? rec["by_account"] : {};
  for (const [aid, n] of Object.entries(raw)) {
    if (typeof n === "number" && n > 0) byAccount[aid] = Math.floor(n);
  }
  const total =
    typeof rec["total"] === "number" ? Math.max(0, Math.floor(rec["total"])) : sumUnread(byAccount);
  return { total, byAccount };
}

export function sumUnread(byAccount: Record<string, number>): number {
  return Object.values(byAccount).reduce((acc, n) => acc + n, 0);
}

export interface QuickReplyDto {
  id: string;
  body: string;
  createdAt: string;
}

export function parseQuickReply(v: unknown): QuickReplyDto {
  if (!isRecord(v)) throw new Error("快捷回复格式错误");
  return {
    id: String(v["id"] ?? ""),
    body: String(v["body"] ?? ""),
    createdAt: String(v["created_at"] ?? "")
  };
}

/** GET 买家备注响应 {note};缺失/空 → 空串(无备注)。 */
export function parseNoteResponse(v: unknown): string {
  if (!isRecord(v)) return "";
  return typeof v["note"] === "string" ? v["note"] : "";
}

// ===== 出站状态视图(FR-041 四状态渲染)=====

export interface OutgoingStatusView {
  /** 展示标记(单勾/图标字符)。 */
  mark: string;
  label: string;
  tone: "normal" | "warning" | "danger" | "neutral";
  /** 仅 failed 可人工重试(新 attempt);uncertain 重试禁用并说明。 */
  canRetry: boolean;
  retryDisabledReason: string;
}

const STATUS_VIEWS: Record<OutgoingStatus, OutgoingStatusView> = {
  sending: {
    mark: "◷",
    label: "发送中",
    tone: "neutral",
    canRetry: false,
    retryDisabledReason: "发送仍在途,等待平台回执"
  },
  sent: {
    mark: "✓",
    label: "已发送",
    tone: "normal",
    canRetry: false,
    retryDisabledReason: "已确认送达,无需重发"
  },
  failed: {
    mark: "!",
    label: "发送失败",
    tone: "danger",
    canRetry: true,
    retryDisabledReason: ""
  },
  uncertain: {
    mark: "⚠",
    label: "结果未知",
    tone: "warning",
    canRetry: false,
    retryDisabledReason: "结果未知,不自动重发,请人工核对"
  },
  cancelled: {
    mark: "—",
    label: "已撤销",
    tone: "neutral",
    canRetry: false,
    retryDisabledReason: "已撤销的发送不可重试"
  }
};

export function statusView(status: OutgoingStatus | null): OutgoingStatusView | null {
  if (status === null) return null;
  return STATUS_VIEWS[status];
}

// ===== 消息合并(3s 轮询窗口刷新;FR-046)=====

/** 按 id 去重合并(新值覆盖同 id 行的状态/回执),按 id 升序排列
 *  (chat_messages.id 为时间有序标识,字典序即时间序——repos 同口径);
 *  保留"加载更早"已翻出的历史页,不被最新窗口截断。 */
export function mergeMessages(
  existing: readonly ChatMessageDto[],
  incoming: readonly ChatMessageDto[]
): ChatMessageDto[] {
  const map = new Map<string, ChatMessageDto>();
  for (const m of existing) map.set(m.id, m);
  for (const m of incoming) map.set(m.id, m);
  return [...map.values()].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

// ===== 展示派生 =====

/** 未读徽标文案:99 封顶(账号 Tab 与会话项共用)。 */
export function unreadBadgeText(n: number): string {
  return n >= 100 ? "99+" : String(Math.max(0, Math.floor(n)));
}

/** 会话对方昵称:昵称缺失降级买家 ID(不显示空白)。 */
export function peerDisplayName(c: ConversationDto): string {
  const name = c.peerNickname?.trim() ?? "";
  return name.length > 0 ? name : c.peerBuyerId;
}

/** 图片消息可直连渲染判定:仅 http(s)/data URL 交给 <img>;
 *  本地数据目录相对路径(uploads/chat/…)无 HTTP 静态路由,如实降级为占位。 */
export function isRenderableImageSrc(path: string | null): boolean {
  if (path === null) return false;
  return /^(https?:|data:image\/)/i.test(path);
}

/** 商品卡片快照摘要(字段名宽松匹配,缺失返回 null;不冒充商品信息)。 */
export interface ItemCardView {
  title: string;
  price: string | null;
  imageUrl: string | null;
}

export function itemCardView(snapshot: Record<string, unknown> | null): ItemCardView | null {
  if (snapshot === null) return null;
  const title = String(snapshot["title"] ?? snapshot["item_title"] ?? "").trim();
  const priceRaw = snapshot["price"] ?? snapshot["price_text"];
  const price = priceRaw === undefined || priceRaw === null ? null : String(priceRaw);
  const picRaw = snapshot["pic_path"] ?? snapshot["pic"] ?? snapshot["image_url"];
  const pic = typeof picRaw === "string" && picRaw.length > 0 ? picRaw : null;
  if (title.length === 0 && price === null && pic === null) return null;
  return { title: title.length > 0 ? title : "(未提供标题)", price, imageUrl: pic };
}

// ===== 输入校验(与后端 domain 同口径:按 unicode 标量计数)=====

export type ValidateResult = { ok: true } | { ok: false; error: string };

function charCount(s: string): number {
  return [...s].length;
}

/** 出站文本:1—2000 标量(空文本不是一条消息;FR-041)。 */
export function validateOutgoingText(text: string): ValidateResult {
  const n = charCount(text);
  if (n === 0) return { ok: false, error: "消息内容不能为空" };
  if (n > MAX_MESSAGE_CHARS) {
    return { ok: false, error: `消息内容超过 ${MAX_MESSAGE_CHARS} 字(当前 ${n} 字)` };
  }
  return { ok: true };
}

/** 快捷回复正文:1—2000 标量。 */
export function validateQuickReplyBody(body: string): ValidateResult {
  const n = charCount(body);
  if (n === 0) return { ok: false, error: "快捷回复内容不能为空" };
  if (n > MAX_MESSAGE_CHARS) {
    return { ok: false, error: `快捷回复内容超过 ${MAX_MESSAGE_CHARS} 字(当前 ${n} 字)` };
  }
  return { ok: true };
}

/** 买家备注:0—2000 标量(空串=清除备注,合法)。 */
export function validateBuyerNote(note: string): ValidateResult {
  const n = charCount(note);
  if (n > MAX_NOTE_CHARS) {
    return { ok: false, error: `买家备注超过 ${MAX_NOTE_CHARS} 字(当前 ${n} 字)` };
  }
  return { ok: true };
}

/** 快捷回复上限守卫:达到 50 条后新增入口禁用(第 51 条由后端 422 拒绝)。 */
export function quickReplyCapReached(current: number): boolean {
  return current >= MAX_QUICK_REPLIES;
}

/** 前端图片预检:扩展名白名单 + ≤10MB(413/非法类型的即时反馈;服务端为准)。 */
export function validateImageFile(name: string, size: number): ValidateResult {
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
  if (!IMAGE_EXTS.includes(ext)) {
    return { ok: false, error: `仅支持 ${IMAGE_EXTS.join("/")} 图片` };
  }
  if (size > IMAGE_MAX_BYTES) {
    return { ok: false, error: "图片超过 10MB 上限" };
  }
  return { ok: true };
}

// ===== 请求串构建 =====

/** 会话列表查询串(search/unread_only/cursor;分页参数名与 repos 一致)。 */
export function sessionsQuery(search: string, unreadOnly: boolean, cursor: string | null): string {
  const sp = new URLSearchParams();
  const q = search.trim();
  if (q.length > 0) sp.set("search", q);
  if (unreadOnly) sp.set("unread_only", "true");
  if (cursor !== null && cursor.length > 0) sp.set("cursor", cursor);
  sp.set("limit", "50");
  return sp.toString();
}

/** 在线聊天视图模型测试(007 T054):MessageDto 契约解析(四状态/五类型/降级)、
 *  会话解析、未读聚合派生、输入校验(2000/空)、能力门禁布尔降级、
 *  快捷回复上限逻辑,以及消息合并/徽标/状态视图等纯函数。 */
import { describe, expect, it } from "vitest";
import { parseChatCapabilities } from "../../shared/contracts";
import {
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
  sumUnread,
  unreadBadgeText,
  validateBuyerNote,
  validateImageFile,
  validateOutgoingText,
  validateQuickReplyBody,
  type ChatMessageDto
} from "./model";

function message(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "msg-1",
    conversation_id: "conv-1",
    direction: "in",
    msg_kind: "text",
    body_text: "你好",
    image_path: null,
    status: null,
    error_hint: null,
    item_snapshot: null,
    created_at: "2026-01-01T00:00:00Z",
    read_at: null,
    ...over
  };
}

describe("parseMessage 契约解析(T054:四状态/五类型/降级)", () => {
  it("入站文本完整解析(transport message_dto 形状)", () => {
    const m = parseMessage(message());
    expect(m.direction).toBe("in");
    expect(m.kind).toBe("text");
    expect(m.bodyText).toBe("你好");
    expect(m.status).toBeNull(); // 入站无状态
  });

  it("出站四状态 + cancelled 全部解析", () => {
    for (const [status, tone] of [
      ["sending", "neutral"],
      ["sent", "normal"],
      ["failed", "danger"],
      ["uncertain", "warning"],
      ["cancelled", "neutral"]
    ] as const) {
      const m = parseMessage(message({ direction: "out", status }));
      expect(m.status).toBe(status);
      expect(m.direction).toBe("out");
      expect(statusView(m.status)?.tone).toBe(tone);
    }
    // failed 可人工重试;uncertain 不可且带禁用说明(红线)
    expect(statusView("failed")?.canRetry).toBe(true);
    const uncertain = statusView("uncertain");
    expect(uncertain?.canRetry).toBe(false);
    expect(uncertain?.retryDisabledReason).toContain("结果未知");
    expect(statusView(null)).toBeNull();
  });

  it("五消息类型全解析;image/system/item_card/unknown 各取其用", () => {
    expect(parseMessage(message({ msg_kind: "image", image_path: "https://cdn.example.com/a.png", body_text: null })).kind).toBe("image");
    expect(parseMessage(message({ msg_kind: "system", body_text: "买家已付款" })).kind).toBe("system");
    const card = parseMessage(
      message({ msg_kind: "item_card", item_snapshot: { title: "网课A", price: "¥99", pic_path: "https://cdn.example.com/p.jpg" } })
    );
    expect(card.kind).toBe("item_card");
    expect(itemCardView(card.itemSnapshot)).toEqual({
      title: "网课A",
      price: "¥99",
      imageUrl: "https://cdn.example.com/p.jpg"
    });
    expect(parseMessage(message({ msg_kind: "unknown", body_text: null })).kind).toBe("unknown");
  });

  it("降级:未知类型→unknown;未知状态→null;非法方向→in;可空字段缺失为 null", () => {
    const m = parseMessage(message({ msg_kind: "voice", status: "queued", direction: "sideways" }));
    expect(m.kind).toBe("unknown");
    expect(m.status).toBeNull();
    expect(m.direction).toBe("in");
    expect(() => parseMessage("nope")).toThrow();
    expect(() => parseMessage(null)).toThrow();
  });

  it("图片可直连判定:仅 http(s)/data URL;<img> 不接本地数据目录路径", () => {
    expect(isRenderableImageSrc("https://a.example.com/x.png")).toBe(true);
    expect(isRenderableImageSrc("http://a.example.com/x.png")).toBe(true);
    expect(isRenderableImageSrc("data:image/png;base64,AAAA")).toBe(true);
    expect(isRenderableImageSrc("uploads/chat/abc.png")).toBe(false);
    expect(isRenderableImageSrc(null)).toBe(false);
    expect(isRenderableImageSrc("/api/attachments/1")).toBe(false);
  });

  it("itemCardView:字段缺失降级(全空 → null 不冒充商品)", () => {
    expect(itemCardView(null)).toBeNull();
    expect(itemCardView({})).toBeNull();
    const partial = itemCardView({ item_title: "仅标题" });
    expect(partial?.title).toBe("仅标题");
    expect(partial?.price).toBeNull();
    expect(partial?.imageUrl).toBeNull();
  });
});

describe("parseConversation 会话解析(T054)", () => {
  it("完整载荷解析;昵称缺失时展示名降级买家 ID", () => {
    const c = parseConversation({
      id: "conv-9",
      account_id: "acct-1",
      peer_buyer_id: "b123@goofish",
      peer_nickname: "小明",
      chat_id: "chat-1",
      last_item_id: "item-7",
      last_message_at: "2026-01-02T00:00:00Z",
      last_message_preview: "发货了吗",
      unread_count: 3
    });
    expect(c.unreadCount).toBe(3);
    expect(peerDisplayName(c)).toBe("小明");
    const bare = parseConversation({ id: "conv-8", account_id: "acct-1", peer_buyer_id: "b456", unread_count: 0 });
    expect(peerDisplayName(bare)).toBe("b456");
    expect(bare.lastMessagePreview).toBeNull();
  });

  it("降级:unread_count 非数字按 0;可空字段为 null;非对象抛错", () => {
    const c = parseConversation({ id: "c", account_id: "a", peer_buyer_id: "b", unread_count: "x" });
    expect(c.unreadCount).toBe(0);
    expect(() => parseConversation(42)).toThrow();
  });
});

describe("未读聚合派生(T054)", () => {
  it("unread-summary:total + by_account;非数字账号项剔除", () => {
    const s = parseUnreadSummary({ total: 5, by_account: { a1: 3, a2: 2, bad: "x", zero: 0 } });
    expect(s.total).toBe(5);
    expect(s.byAccount).toEqual({ a1: 3, a2: 2 });
  });

  it("total 缺失时按 by_account 求和派生(不冒充 0);空载荷安全", () => {
    expect(parseUnreadSummary({ by_account: { a1: 3, a2: 2 } }).total).toBe(5);
    expect(parseUnreadSummary({}).total).toBe(0);
    expect(parseUnreadSummary({ total: -1 }).total).toBe(0);
    expect(sumUnread({ a: 2, b: 3 })).toBe(5);
  });

  it("徽标文案:99 封顶 99+;0/负数不越界", () => {
    expect(unreadBadgeText(0)).toBe("0");
    expect(unreadBadgeText(99)).toBe("99");
    expect(unreadBadgeText(100)).toBe("99+");
    expect(unreadBadgeText(1234)).toBe("99+");
    expect(unreadBadgeText(-3)).toBe("0");
  });
});

describe("输入校验(T054:2000/空;与后端 domain 同口径)", () => {
  it("出站文本:空拒绝;2000 合法;2001 拒绝;按标量计数", () => {
    expect(validateOutgoingText("").ok).toBe(false);
    expect(validateOutgoingText("   ").ok).toBe(true); // 空白非空(后端同口径:仅拒空串)
    expect(validateOutgoingText("字".repeat(MAX_MESSAGE_CHARS)).ok).toBe(true);
    const over = validateOutgoingText("字".repeat(MAX_MESSAGE_CHARS + 1));
    expect(over.ok).toBe(false);
    if (!over.ok) expect(over.error).toContain("2000");
    // 多字节字符不放大计数(JS 迭代器=unicode 标量)
    expect(validateOutgoingText("あ".repeat(MAX_MESSAGE_CHARS)).ok).toBe(true);
  });

  it("快捷回复正文:空拒绝/2001 拒绝;买家备注:空串合法/2001 拒绝", () => {
    expect(validateQuickReplyBody("").ok).toBe(false);
    expect(validateQuickReplyBody("已发货").ok).toBe(true);
    expect(validateQuickReplyBody("字".repeat(MAX_MESSAGE_CHARS + 1)).ok).toBe(false);
    expect(validateBuyerNote("").ok).toBe(true); // 空串=清除备注
    const note = validateBuyerNote("字".repeat(MAX_NOTE_CHARS + 1));
    expect(note.ok).toBe(false);
    if (!note.ok) expect(note.error).toContain("2000");
  });

  it("图片预检:扩展名白名单与 10MB 上限(服务端为准的即时反馈)", () => {
    expect(validateImageFile("a.png", 1024).ok).toBe(true);
    expect(validateImageFile("a.JPG", 1024).ok).toBe(true); // 大小写不敏感
    expect(validateImageFile("a.bmp", 1024).ok).toBe(false);
    expect(validateImageFile("noext", 1024).ok).toBe(false);
    expect(validateImageFile("a.png", 10 * 1024 * 1024 + 1).ok).toBe(false);
  });
});

describe("能力门禁布尔降级(T054,FR-042)", () => {
  it("chat_send_image/chat_history_backfill 显式 true 才开;缺失/类型异常一律 false", () => {
    expect(parseChatCapabilities({ chat_send_image: true, chat_history_backfill: true })).toEqual({
      chatSendImage: true,
      chatHistoryBackfill: true
    });
    expect(parseChatCapabilities({})).toEqual({ chatSendImage: false, chatHistoryBackfill: false });
    expect(parseChatCapabilities({ chat_send_image: "yes", chat_history_backfill: 1 })).toEqual({
      chatSendImage: false,
      chatHistoryBackfill: false
    });
    expect(parseChatCapabilities(null)).toEqual({ chatSendImage: false, chatHistoryBackfill: false });
  });
});

describe("快捷回复上限逻辑(T054,FR-044:50 条)", () => {
  it("第 50 条到达上限;49 条未达;上限提示常量与后端一致", () => {
    expect(MAX_QUICK_REPLIES).toBe(50);
    expect(quickReplyCapReached(0)).toBe(false);
    expect(quickReplyCapReached(49)).toBe(false);
    expect(quickReplyCapReached(50)).toBe(true);
    expect(quickReplyCapReached(51)).toBe(true);
  });

  it("parseQuickReply/parseNoteResponse:正常与降级", () => {
    const q = parseQuickReply({ id: "qr-1", body: "感谢惠顾", created_at: "t" });
    expect(q.body).toBe("感谢惠顾");
    expect(parseQuickReply({ id: "qr-2" }).body).toBe("");
    expect(() => parseQuickReply([])).toThrow();
    expect(parseNoteResponse({ note: "老买家" })).toBe("老买家");
    expect(parseNoteResponse({ note: null })).toBe("");
    expect(parseNoteResponse("bad")).toBe("");
  });
});

describe("消息合并与查询串(T054:3s 轮询窗口合并/游标参数)", () => {
  const dto = (id: string, over: Partial<ChatMessageDto> = {}): ChatMessageDto => ({
    id,
    conversationId: "conv-1",
    direction: "out",
    kind: "text",
    bodyText: `t-${id}`,
    imagePath: null,
    status: "sending",
    errorHint: null,
    itemSnapshot: null,
    createdAt: "t",
    readAt: null,
    ...over
  });

  it("mergeMessages:同 id 新值覆盖(sending→sent 刷新);按 id 升序去重", () => {
    const existing = [dto("a"), dto("b", { status: "sending" }), dto("c")];
    const incoming = [dto("d"), dto("b", { status: "sent" })]; // API 返回新→旧乱序也稳
    const merged = mergeMessages(existing, incoming);
    expect(merged.map((m) => m.id)).toEqual(["a", "b", "c", "d"]);
    expect(merged.find((m) => m.id === "b")?.status).toBe("sent");
  });

  it("mergeMessages:保留已翻出的更早页(历史页不被最新窗口截断)", () => {
    const olderPage = [dto("a"), dto("b")];
    const windowPage = [dto("y"), dto("z")];
    expect(mergeMessages([...olderPage, ...windowPage], windowPage)).toHaveLength(4);
    expect(mergeMessages([], windowPage).map((m) => m.id)).toEqual(["y", "z"]);
  });

  it("sessionsQuery:search/unread_only/cursor/limit 参数名与 repos 一致", () => {
    expect(sessionsQuery("", false, null)).toBe("limit=50");
    expect(sessionsQuery(" 小明 ", true, null)).toBe("search=%E5%B0%8F%E6%98%8E&unread_only=true&limit=50");
    expect(sessionsQuery("", false, "t|c|id")).toBe("cursor=t%7Cc%7Cid&limit=50");
  });
});

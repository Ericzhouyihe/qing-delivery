/** 通知设置视图模型测试(007 T065):渠道契约解析(七类型/kind 降级/secrets_configured
 *  /event_types 非法项忽略)、订阅摘要(空=全部)、绑定映射(去重/摘要)、
 *  表单校验与载荷构建(URL 形状/端口范围/秘密留空=不修改)、事件枚举映射
 *  ——与后端 domain/notify.rs、application/notify 同口径。 */
import { describe, expect, it } from "vitest";
import {
  CHANNEL_KINDS,
  EVENT_TYPES,
  buildChannelPayload,
  buildSystemSmtpPayload,
  bindingSummary,
  configSummary,
  isChannelKind,
  isEventType,
  parseBindings,
  parseChannel,
  parseSystemSmtp,
  parseTestSendResult,
  subscriptionSummary,
  testResultText,
  testResultTone,
  uniqueChannelIds,
  validateChannelName,
  validateChatId,
  validateEmailAddress,
  validateHttpUrl,
  validatePort,
  type ChannelFormInput,
  type EventType
} from "./model";

function form(over: Partial<ChannelFormInput> = {}): ChannelFormInput {
  return {
    kind: "webhook",
    name: "值班通知",
    enabled: true,
    url: "https://example.com/hook",
    secret: "",
    serverUrl: "",
    deviceKey: "",
    botToken: "",
    chatId: "",
    toAddress: "",
    useSystemSmtp: true,
    smtpHost: "",
    smtpPort: "",
    smtpUser: "",
    smtpPassword: "",
    fromAddress: "",
    eventTypes: [],
    ...over
  };
}

describe("parseChannel 契约解析(T064/T065)", () => {
  it("七类型渠道完整解析(transport ChannelSummary 形状)", () => {
    const raw = {
      id: "nch-1",
      kind: "dingtalk",
      name: "值班群",
      enabled: true,
      config: { url: "https://oapi.dingtalk.com/robot/send?access_token=x" },
      secrets_configured: ["secret"],
      event_types: ["account_offline", "delivery_result"],
      version: 3
    };
    const v = parseChannel(raw);
    expect(v.kind).toBe("dingtalk");
    expect(v.kindRaw).toBe("dingtalk");
    expect(v.config["url"]).toContain("oapi.dingtalk.com");
    expect(v.secretsConfigured).toEqual(["secret"]);
    expect(v.eventTypes).toEqual(["account_offline", "delivery_result"]);
    expect(v.version).toBe(3);
    // 七类型逐一可通过 kind 判定(枚举映射)
    for (const k of CHANNEL_KINDS) {
      expect(parseChannel({ id: "x", kind: k }).kind).toBe(k);
    }
  });

  it("字段缺失降级:enabled/config/event_types/secrets_configured 不冒充真实值", () => {
    const v = parseChannel({ id: "nch-2", kind: "webhook", name: "最小载荷" });
    expect(v.enabled).toBe(false);
    expect(v.config).toEqual({});
    expect(v.secretsConfigured).toEqual([]);
    expect(v.eventTypes).toEqual([]);
    expect(v.version).toBe(1); // 缺失按 1,不臆造高版本
  });

  it("kind 未知值降级 webhook 且 kindRaw 保留原值;event_types 非法项忽略", () => {
    const v = parseChannel({
      id: "nch-3",
      kind: "sms",
      name: "未知类型",
      event_types: ["account_offline", "order_paid", 42]
    });
    expect(v.kind).toBe("webhook");
    expect(v.kindRaw).toBe("sms");
    expect(v.eventTypes).toEqual(["account_offline"]); // 宽松解析:非法项忽略
  });

  it("非对象抛错", () => {
    expect(() => parseChannel("nope")).toThrow();
    expect(() => parseChannel(null)).toThrow();
    expect(() => parseChannel([])).toThrow();
  });
});

describe("事件枚举与订阅摘要(T065)", () => {
  it("六事件枚举映射:合法值判定与 wire 值一致", () => {
    expect(EVENT_TYPES).toHaveLength(6);
    for (const t of EVENT_TYPES) {
      expect(isEventType(t)).toBe(true);
    }
    expect(isEventType("order_paid")).toBe(false);
    expect(isEventType("")).toBe(false);
    // 渠道七枚举
    expect(CHANNEL_KINDS).toHaveLength(7);
    for (const k of CHANNEL_KINDS) {
      expect(isChannelKind(k)).toBe(true);
    }
    expect(isChannelKind("sms")).toBe(false);
  });

  it("订阅摘要:空 = 全部事件;否则中文标签拼接(FR-051 明示)", () => {
    expect(subscriptionSummary([])).toBe("全部事件");
    const subs: EventType[] = ["account_offline", "delivery_result"];
    expect(subscriptionSummary(subs)).toBe("账号掉线、交付结果");
    expect(subscriptionSummary(["manual_intervention_required"])).toBe("需要人工处理");
  });

  it("载荷构建:全不勾选序列化为空数组(后端空=订阅全部)", () => {
    const built = buildChannelPayload(form({ eventTypes: [] }), { secretsRequired: false });
    expect(built.ok).toBe(true);
    if (built.ok) expect(built.payload.event_types).toEqual([]);
    const all = buildChannelPayload(
      form({ eventTypes: ["account_offline", "security_verification"] }),
      { secretsRequired: false }
    );
    expect(all.ok).toBe(true);
    if (all.ok) {
      expect(all.payload.event_types).toEqual(["account_offline", "security_verification"]);
    }
  });
});

describe("绑定映射(T065,FR-052 覆盖式)", () => {
  it("parseBindings:channel_ids 数组解析;非对象抛错", () => {
    expect(parseBindings({ channel_ids: ["a", "b"] })).toEqual(["a", "b"]);
    expect(parseBindings({ channel_ids: [] })).toEqual([]);
    expect(() => parseBindings({})).toThrow();
    expect(() => parseBindings("x")).toThrow();
  });

  it("uniqueChannelIds:trim、去空、去重保序(覆盖式保存前的整理)", () => {
    expect(uniqueChannelIds([" a ", "", "b", "a", " ", "c"])).toEqual(["a", "b", "c"]);
    expect(uniqueChannelIds([])).toEqual([]);
  });

  it("bindingSummary:空 = 未绑定走全部启用渠道;非空计数", () => {
    expect(bindingSummary([])).toBe("未绑定 · 发送至全部启用渠道");
    expect(bindingSummary(["nch-1"])).toBe("已绑定 1 个渠道");
    expect(bindingSummary(["nch-1", "nch-2"])).toBe("已绑定 2 个渠道");
  });
});

describe("表单校验:URL 形状 / 端口范围 / 邮箱 / chat_id(T065)", () => {
  it("validateHttpUrl:空、非 URL、非 http(s)、缺主机拒绝;合法通过", () => {
    expect(validateHttpUrl("", "回调 URL").ok).toBe(false);
    expect(validateHttpUrl("not a url", "回调 URL").ok).toBe(false);
    expect(validateHttpUrl("ftp://example.com", "回调 URL").ok).toBe(false);
    expect(validateHttpUrl("http://", "回调 URL").ok).toBe(false);
    expect(validateHttpUrl("https://oapi.dingtalk.com/robot/send?token=x", "回调 URL").ok).toBe(true);
    expect(validateHttpUrl("  http://a.b/c ", "回调 URL").ok).toBe(true);
  });

  it("validatePort:整数 1—65535;0/65536/小数/非数字拒绝", () => {
    expect(validatePort("587")).toEqual({ ok: true, value: 587, message: "" });
    expect(validatePort("1").ok).toBe(true);
    expect(validatePort("65535")).toEqual({ ok: true, value: 65535, message: "" });
    expect(validatePort("0").ok).toBe(false);
    expect(validatePort("65536").ok).toBe(false);
    expect(validatePort("abc").ok).toBe(false);
    expect(validatePort("58.7").ok).toBe(false);
    expect(validatePort("-1").ok).toBe(false);
    expect(validatePort(" 465 ").value).toBe(465);
  });

  it("validateEmailAddress / validateChatId / validateChannelName 边界", () => {
    expect(validateEmailAddress("a@b.c", "收件邮箱").ok).toBe(true);
    expect(validateEmailAddress("ab.c", "收件邮箱").ok).toBe(false);
    expect(validateEmailAddress("", "收件邮箱").ok).toBe(false);
    // chat_id:数字(含负数)/@频道名/- 开头群组(与后端口径一致)
    expect(validateChatId("123456").ok).toBe(true);
    expect(validateChatId("-100123456").ok).toBe(true);
    expect(validateChatId("@channel").ok).toBe(true);
    expect(validateChatId("").ok).toBe(false);
    expect(validateChatId("hello").ok).toBe(false);
    expect(validateChannelName("  值班群 ").ok).toBe(true);
    expect(validateChannelName("   ").ok).toBe(false);
    expect(validateChannelName("a".repeat(101)).ok).toBe(false);
  });
});

describe("buildChannelPayload:各类型 config/秘密构建与秘密留空语义(T065)", () => {
  it("webhook:合法 URL 进 config;无秘密键", () => {
    const built = buildChannelPayload(form({ kind: "webhook" }), { secretsRequired: true });
    expect(built.ok).toBe(true);
    if (built.ok) {
      expect(built.payload.config).toEqual({ url: "https://example.com/hook" });
      expect(built.payload.secrets).toEqual({});
    }
    expect(buildChannelPayload(form({ kind: "webhook", url: "" }), { secretsRequired: true }).ok).toBe(false);
  });

  it("dingtalk/feishu:加签 secret 可选——创建留空合法,填了才进 secrets 包", () => {
    const blank = buildChannelPayload(form({ kind: "dingtalk", secret: "" }), { secretsRequired: true });
    expect(blank.ok).toBe(true);
    if (blank.ok) expect(blank.payload.secrets).toEqual({});
    const signed = buildChannelPayload(form({ kind: "feishu", secret: "  sec123  " }), {
      secretsRequired: false
    });
    expect(signed.ok).toBe(true);
    if (signed.ok) {
      expect(signed.payload.secrets).toEqual({ secret: "sec123" }); // trim 后入包
      expect(signed.payload.kind).toBe("feishu");
    }
  });

  it("bark:创建必填 device_key;编辑(secretsRequired=false)留空=不修改(空包)", () => {
    const missing = buildChannelPayload(
      form({ kind: "bark", serverUrl: "https://api.day.app", deviceKey: "" }),
      { secretsRequired: true }
    );
    expect(missing.ok).toBe(false);
    if (!missing.ok) expect(missing.error).toContain("device_key");
    // 编辑:留空放行且 secrets 为空包(后端沿用旧包)
    const edit = buildChannelPayload(
      form({ kind: "bark", serverUrl: "https://api.day.app", deviceKey: "" }),
      { secretsRequired: false }
    );
    expect(edit.ok).toBe(true);
    if (edit.ok) {
      expect(edit.payload.secrets).toEqual({});
      expect(edit.payload.config).toEqual({ server_url: "https://api.day.app" });
    }
    const create = buildChannelPayload(
      form({ kind: "bark", serverUrl: "https://api.day.app", deviceKey: "key-1" }),
      { secretsRequired: true }
    );
    expect(create.ok).toBe(true);
    if (create.ok) expect(create.payload.secrets).toEqual({ device_key: "key-1" });
  });

  it("telegram:创建必填 bot_token;chat_id 形状随载荷校验", () => {
    const missing = buildChannelPayload(form({ kind: "telegram", chatId: "@news" }), {
      secretsRequired: true
    });
    expect(missing.ok).toBe(false);
    if (!missing.ok) expect(missing.error).toContain("bot_token");
    const ok = buildChannelPayload(
      form({ kind: "telegram", chatId: "-100123456", botToken: "123:ABC" }),
      { secretsRequired: true }
    );
    expect(ok.ok).toBe(true);
    if (ok.ok) {
      expect(ok.payload.config).toEqual({ chat_id: "-100123456" });
      expect(ok.payload.secrets).toEqual({ bot_token: "123:ABC" });
    }
    const badChat = buildChannelPayload(
      form({ kind: "telegram", chatId: "hello", botToken: "123:ABC" }),
      { secretsRequired: false }
    );
    expect(badChat.ok).toBe(false);
  });

  it("email:系统 SMTP 只需收件邮箱;独立 SMTP 需 host+发件地址,端口非法拒绝", () => {
    const sys = buildChannelPayload(
      form({ kind: "email", toAddress: "a@b.c", useSystemSmtp: true }),
      { secretsRequired: true }
    );
    expect(sys.ok).toBe(true);
    if (sys.ok) {
      expect(sys.payload.config).toEqual({ to_address: "a@b.c", use_system_smtp: true });
    }
    // 未启用系统 SMTP 且缺 host/from → 拒绝并列出缺失项
    const bad = buildChannelPayload(
      form({ kind: "email", toAddress: "a@b.c", useSystemSmtp: false }),
      { secretsRequired: false }
    );
    expect(bad.ok).toBe(false);
    if (!bad.ok) {
      expect(bad.error).toContain("SMTP 服务器");
      expect(bad.error).toContain("发件地址");
    }
    const standalone = buildChannelPayload(
      form({
        kind: "email",
        toAddress: "a@b.c",
        useSystemSmtp: false,
        smtpHost: "smtp.example.com",
        smtpPort: "465",
        fromAddress: "shop@example.com",
        smtpUser: "u",
        smtpPassword: "p"
      }),
      { secretsRequired: false }
    );
    expect(standalone.ok).toBe(true);
    if (standalone.ok) {
      expect(standalone.payload.config).toEqual({
        to_address: "a@b.c",
        use_system_smtp: false,
        smtp_host: "smtp.example.com",
        smtp_port: 465,
        from_address: "shop@example.com"
      });
      expect(standalone.payload.secrets).toEqual({ smtp_user: "u", smtp_password: "p" });
    }
    const badPort = buildChannelPayload(
      form({
        kind: "email",
        toAddress: "a@b.c",
        useSystemSmtp: false,
        smtpHost: "smtp.example.com",
        smtpPort: "99999",
        fromAddress: "shop@example.com"
      }),
      { secretsRequired: false }
    );
    expect(badPort.ok).toBe(false);
    if (!badPort.ok) expect(badPort.error).toContain("端口");
  });

  it("秘密留空=空包语义:webhook/wecom 永无秘密键;名称非法优先拦截", () => {
    const wecom = buildChannelPayload(form({ kind: "wecom" }), { secretsRequired: true });
    expect(wecom.ok).toBe(true);
    if (wecom.ok) expect(wecom.payload.secrets).toEqual({});
    const badName = buildChannelPayload(form({ name: "   " }), { secretsRequired: false });
    expect(badName.ok).toBe(false);
    if (!badName.ok) expect(badName.error).toContain("名称");
  });
});

describe("测试投递与系统 SMTP 解析(T064/T065)", () => {
  it("parseTestSendResult:三态与 error_hint;未知 state 降级 unknown", () => {
    expect(parseTestSendResult({ state: "accepted" })).toEqual({
      state: "accepted",
      errorHint: null
    });
    expect(parseTestSendResult({ state: "not_sent", error_hint: "加签错误" })).toEqual({
      state: "not_sent",
      errorHint: "加签错误"
    });
    expect(parseTestSendResult({ state: "whatever" }).state).toBe("unknown");
    expect(() => parseTestSendResult("x")).toThrow();
  });

  it("testResultText/Tone:成功=normal;未发送=danger 带原因;未知=warning 兜底文案", () => {
    expect(testResultText({ state: "accepted", errorHint: null })).toBe("✓ 测试投递成功");
    expect(testResultText({ state: "not_sent", errorHint: "网络不可达" })).toBe("✗ 未发送:网络不可达");
    expect(testResultText({ state: "not_sent", errorHint: null })).toContain("无失败原因");
    expect(testResultText({ state: "unknown", errorHint: "超时" })).toBe("⚠ 结果未知:超时");
    expect(testResultTone({ state: "accepted", errorHint: null })).toBe("tone-normal");
    expect(testResultTone({ state: "not_sent", errorHint: null })).toBe("tone-danger");
    expect(testResultTone({ state: "unknown", errorHint: null })).toBe("tone-warning");
  });

  it("parseSystemSmtp:完整载荷与默认降级(端口 587/字段空串)", () => {
    const v = parseSystemSmtp({
      configured: true,
      host: "smtp.example.com",
      port: 465,
      encryption: "ssl",
      from_address: "n@example.com",
      from_name: "轻交付",
      username: "u",
      password_configured: true
    });
    expect(v).toEqual({
      configured: true,
      host: "smtp.example.com",
      port: 465,
      encryption: "ssl",
      fromAddress: "n@example.com",
      fromName: "轻交付",
      username: "u",
      passwordConfigured: true
    });
    const empty = parseSystemSmtp({});
    expect(empty.configured).toBe(false);
    expect(empty.port).toBe(587); // 后端缺省口径
    expect(empty.fromName).toBeNull();
    expect(empty.username).toBeNull();
    expect(empty.passwordConfigured).toBe(false);
    expect(() => parseSystemSmtp("x")).toThrow();
  });

  it("buildSystemSmtpPayload:密码留空不带 password 键(=不修改);非法端口/加密拒绝", () => {
    const keep = buildSystemSmtpPayload({
      host: "smtp.example.com",
      port: "587",
      encryption: "starttls",
      username: "u",
      password: "",
      fromName: "轻交付",
      fromAddress: "n@example.com"
    });
    expect(keep.ok).toBe(true);
    if (keep.ok) {
      expect(keep.payload.password).toBeUndefined();
      expect(keep.payload).toEqual({
        host: "smtp.example.com",
        port: 587,
        encryption: "starttls",
        from_address: "n@example.com",
        from_name: "轻交付",
        username: "u"
      });
    }
    const changed = buildSystemSmtpPayload({
      host: "smtp.example.com",
      port: "465",
      encryption: "ssl",
      username: "",
      password: "  new-pass  ",
      fromName: "",
      fromAddress: "n@example.com"
    });
    expect(changed.ok).toBe(true);
    if (changed.ok) expect(changed.payload.password).toBe("new-pass");
    // 非法:host 空 / 端口越界 / 加密方式未知 / 发件地址无 @
    expect(
      buildSystemSmtpPayload({ host: " ", port: "587", encryption: "ssl", username: "", password: "", fromName: "", fromAddress: "a@b.c" }).ok
    ).toBe(false);
    expect(
      buildSystemSmtpPayload({ host: "h", port: "0", encryption: "ssl", username: "", password: "", fromName: "", fromAddress: "a@b.c" }).ok
    ).toBe(false);
    expect(
      buildSystemSmtpPayload({ host: "h", port: "587", encryption: "tls", username: "", password: "", fromName: "", fromAddress: "a@b.c" }).ok
    ).toBe(false);
    expect(
      buildSystemSmtpPayload({ host: "h", port: "587", encryption: "ssl", username: "", password: "", fromName: "", fromAddress: "nope" }).ok
    ).toBe(false);
  });
});

describe("configSummary 非机密摘要(T065)", () => {
  it("各类型:URL 取主机;bark 服务器;telegram chat;email 区分系统/独立 SMTP", () => {
    expect(configSummary("webhook", { url: "https://hooks.example.com/abc" })).toBe("hooks.example.com");
    expect(configSummary("dingtalk", { url: "https://oapi.dingtalk.com/robot/send" })).toBe("oapi.dingtalk.com");
    expect(configSummary("wecom", {})).toBe("—");
    expect(configSummary("bark", { server_url: "https://api.day.app" })).toBe("api.day.app");
    expect(configSummary("telegram", { chat_id: "-100123" })).toBe("chat -100123");
    expect(configSummary("email", { to_address: "a@b.c", use_system_smtp: true })).toBe("a@b.c(系统 SMTP)");
    expect(configSummary("email", { to_address: "a@b.c", smtp_host: "smtp.x.com" })).toBe("a@b.c(smtp.x.com)");
    expect(configSummary("email", {})).toBe("—");
    // URL 不可解析时原样回退(不抛错)
    expect(configSummary("webhook", { url: "not a url" })).toBe("not a url");
  });
});

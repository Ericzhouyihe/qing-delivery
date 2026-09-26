/** 通知设置页视图模型(007 T064/T065):渠道契约解析(七类型/降级/secrets_configured)、
 *  事件枚举映射与订阅摘要(空=全部)、账号绑定映射、渠道/系统 SMTP 表单校验与
 *  请求载荷构建(URL 形状/端口范围/秘密留空=不修改)——"算"与"摆"分离,
 *  校验口径与 src/domain/notify.rs、src/application/notify/mod.rs 对齐(宪章 II)。 */
import { isRecord } from "../../shared/contracts";

// ===== 类型词汇表(FR-050 七类渠道 / FR-051 六类事件)=====

export type ChannelKind =
  | "webhook"
  | "email"
  | "dingtalk"
  | "feishu"
  | "wecom"
  | "bark"
  | "telegram";

export const CHANNEL_KINDS: readonly ChannelKind[] = [
  "webhook",
  "email",
  "dingtalk",
  "feishu",
  "wecom",
  "bark",
  "telegram"
];

export type EventType =
  | "account_offline"
  | "account_recovered"
  | "security_verification"
  | "manual_intervention_required"
  | "delivery_result"
  | "system_error";

export const EVENT_TYPES: readonly EventType[] = [
  "account_offline",
  "account_recovered",
  "security_verification",
  "manual_intervention_required",
  "delivery_result",
  "system_error"
];

export function isChannelKind(v: string): v is ChannelKind {
  return (CHANNEL_KINDS as readonly string[]).includes(v);
}

/** 与后端 parse_event_types_lenient 同语义:无法识别的存储值忽略(防御)。 */
export function isEventType(v: string): v is EventType {
  return (EVENT_TYPES as readonly string[]).includes(v);
}

export const KIND_LABELS: Record<ChannelKind, string> = {
  webhook: "Webhook",
  email: "邮件",
  dingtalk: "钉钉",
  feishu: "飞书",
  wecom: "企业微信",
  bark: "Bark",
  telegram: "Telegram"
};

/** 渠道卡类型图标(单字符,与侧边栏图标词汇表同风格)。 */
export const KIND_ICONS: Record<ChannelKind, string> = {
  webhook: "◉",
  email: "✉",
  dingtalk: "◆",
  feishu: "◇",
  wecom: "▣",
  bark: "▲",
  telegram: "✈"
};

export const KIND_TONES: Record<ChannelKind, "info" | "normal" | "neutral"> = {
  webhook: "info",
  email: "normal",
  dingtalk: "info",
  feishu: "info",
  wecom: "info",
  bark: "normal",
  telegram: "normal"
};

export const EVENT_LABELS: Record<EventType, string> = {
  account_offline: "账号掉线",
  account_recovered: "账号恢复",
  security_verification: "安全验证",
  manual_intervention_required: "需要人工处理",
  delivery_result: "交付结果",
  system_error: "系统错误"
};

/** 事件说明(页面说明卡与编辑器订阅组共用;四类自动化失败统一走"需要人工处理")。 */
export const EVENT_NOTES: Record<EventType, string> = {
  account_offline: "账号运行时掉线(连接断开)",
  account_recovered: "离线账号重新恢复在线",
  security_verification: "平台触发安全验证(授权变更)",
  manual_intervention_required: "四类自动化失败转待处理(交付/规则/回复/求评)",
  delivery_result: "交付失败或结果未知(成功不通知)",
  system_error: "系统级错误(协议异常等)"
};

// ===== 契约解析(GET /notification-channels 的 ChannelSummary)=====

export interface NotificationChannelDto {
  id: string;
  /** 未知存储值降级 "webhook"(kindRaw 保留原值供防御展示)。 */
  kind: ChannelKind;
  kindRaw: string;
  name: string;
  enabled: boolean;
  /** 明文配置(后端 config 字段;秘密永不回显)。 */
  config: Record<string, unknown>;
  /** 已配置秘密键名列表(secrets_configured 脱敏回显)。 */
  secretsConfigured: string[];
  /** 订阅事件(存储值非法的项被忽略;空 = 全部,FR-051)。 */
  eventTypes: EventType[];
  version: number;
}

export function parseChannel(v: unknown): NotificationChannelDto {
  if (!isRecord(v)) throw new Error("通知渠道格式错误");
  const kindRaw = String(v["kind"] ?? "");
  const configRaw = v["config"];
  const eventsRaw = Array.isArray(v["event_types"]) ? v["event_types"] : [];
  return {
    id: String(v["id"] ?? ""),
    kind: isChannelKind(kindRaw) ? kindRaw : "webhook",
    kindRaw,
    name: String(v["name"] ?? ""),
    enabled: v["enabled"] === true,
    config: isRecord(configRaw) ? configRaw : {},
    secretsConfigured: (
      Array.isArray(v["secrets_configured"]) ? v["secrets_configured"] : []
    ).map((k) => String(k)),
    eventTypes: eventsRaw.map((e) => String(e)).filter(isEventType),
    version: typeof v["version"] === "number" ? v["version"] : 1
  };
}

// ===== 测试投递结果(POST /notification-channels/{id}/test)=====

export interface TestSendResult {
  state: "accepted" | "not_sent" | "unknown";
  errorHint: string | null;
}

export function parseTestSendResult(v: unknown): TestSendResult {
  if (!isRecord(v)) throw new Error("测试结果格式错误");
  const raw = String(v["state"] ?? "");
  const state: TestSendResult["state"] =
    raw === "accepted" || raw === "not_sent" || raw === "unknown" ? raw : "unknown";
  return {
    state,
    errorHint: typeof v["error_hint"] === "string" ? v["error_hint"] : null
  };
}

/** 行内结果文案:成功/未发送(结构化失败原因)/结果未知。 */
export function testResultText(r: TestSendResult): string {
  if (r.state === "accepted") return "✓ 测试投递成功";
  const hint = r.errorHint !== null && r.errorHint.length > 0 ? r.errorHint : "无失败原因";
  return r.state === "not_sent" ? `✗ 未发送:${hint}` : `⚠ 结果未知:${hint}`;
}

/** 行内结果语义色(tone-* 全局词汇表)。 */
export function testResultTone(r: TestSendResult): "tone-normal" | "tone-danger" | "tone-warning" {
  if (r.state === "accepted") return "tone-normal";
  return r.state === "not_sent" ? "tone-danger" : "tone-warning";
}

// ===== 系统 SMTP(GET/PUT /settings/system-smtp;密码脱敏)=====

export type SmtpEncryption = "none" | "starttls" | "ssl";

export const SMTP_ENCRYPTIONS: ReadonlyArray<{ value: SmtpEncryption; label: string }> = [
  { value: "starttls", label: "STARTTLS(常用 587)" },
  { value: "ssl", label: "SSL/TLS(常用 465)" },
  { value: "none", label: "无加密" }
];

export const DEFAULT_SMTP_PORT = 587;

export interface SystemSmtpDto {
  configured: boolean;
  host: string;
  port: number;
  encryption: string;
  fromAddress: string;
  fromName: string | null;
  username: string | null;
  passwordConfigured: boolean;
}

export function parseSystemSmtp(v: unknown): SystemSmtpDto {
  if (!isRecord(v)) throw new Error("系统 SMTP 配置格式错误");
  return {
    configured: v["configured"] === true,
    host: String(v["host"] ?? ""),
    port: typeof v["port"] === "number" ? v["port"] : DEFAULT_SMTP_PORT,
    encryption: String(v["encryption"] ?? ""),
    fromAddress: String(v["from_address"] ?? ""),
    fromName: typeof v["from_name"] === "string" && v["from_name"].length > 0 ? v["from_name"] : null,
    username: typeof v["username"] === "string" && v["username"].length > 0 ? v["username"] : null,
    passwordConfigured: v["password_configured"] === true
  };
}

// ===== 账号绑定(GET/PUT /accounts/{aid}/notification-bindings,覆盖式)=====

export function parseBindings(v: unknown): string[] {
  if (!isRecord(v) || !Array.isArray(v["channel_ids"])) throw new Error("绑定响应格式错误");
  return v["channel_ids"].map((x) => String(x));
}

/** 覆盖式保存前的整理:trim、去空、去重(保持原顺序)。 */
export function uniqueChannelIds(ids: readonly string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of ids) {
    const id = raw.trim();
    if (id.length === 0 || seen.has(id)) continue;
    seen.add(id);
    out.push(id);
  }
  return out;
}

/** 绑定摘要文案:空 = 未绑定(事件走全部启用渠道,FR-052)。 */
export function bindingSummary(boundIds: readonly string[]): string {
  return boundIds.length === 0 ? "未绑定 · 发送至全部启用渠道" : `已绑定 ${boundIds.length} 个渠道`;
}

// ===== 派生:订阅摘要 / 配置摘要 / 秘密标签 =====

/** 订阅摘要(渠道卡):空 = 全部事件(FR-051"不选=全部"明示)。 */
export function subscriptionSummary(eventTypes: readonly EventType[]): string {
  if (eventTypes.length === 0) return "全部事件";
  return eventTypes.map((e) => EVENT_LABELS[e]).join("、");
}

function cfgStr(config: Record<string, unknown>, key: string): string {
  const v = config[key];
  return typeof v === "string" ? v.trim() : "";
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** 非机密配置摘要(渠道卡一行说明;秘密只显示"已配置"标签,不回显)。 */
export function configSummary(kind: ChannelKind, config: Record<string, unknown>): string {
  switch (kind) {
    case "webhook":
    case "dingtalk":
    case "feishu":
    case "wecom": {
      const url = cfgStr(config, "url");
      return url.length > 0 ? hostOf(url) : "—";
    }
    case "bark": {
      const server = cfgStr(config, "server_url");
      return server.length > 0 ? hostOf(server) : "—";
    }
    case "telegram": {
      const chat = cfgStr(config, "chat_id");
      return chat.length > 0 ? `chat ${chat}` : "—";
    }
    case "email": {
      const to = cfgStr(config, "to_address");
      if (to.length === 0) return "—";
      if (config["use_system_smtp"] === true) return `${to}(系统 SMTP)`;
      const host = cfgStr(config, "smtp_host");
      return host.length > 0 ? `${to}(${host})` : to;
    }
  }
}

export const SECRET_LABELS: Record<string, string> = {
  secret: "加签 secret",
  device_key: "device_key",
  bot_token: "bot_token",
  smtp_user: "SMTP 账号",
  smtp_password: "SMTP 密码"
};

export function secretLabel(key: string): string {
  return SECRET_LABELS[key] ?? key;
}

// ===== 表单校验(与后端同口径;秘密留空=不修改)=====

export function validateChannelName(raw: string): { ok: boolean; message: string } {
  const t = raw.trim();
  if (t.length === 0) return { ok: false, message: "名称不能为空" };
  if ([...t].length > 100) return { ok: false, message: "名称过长(上限 100 字)" };
  return { ok: true, message: "" };
}

/** URL 形状校验(后端 check_http_url 同口径:http/https + 可解析)。 */
export function validateHttpUrl(raw: string, label: string): { ok: boolean; message: string } {
  const t = raw.trim();
  if (t.length === 0) return { ok: false, message: `${label}不能为空` };
  let parsed: URL;
  try {
    parsed = new URL(t);
  } catch {
    return { ok: false, message: `${label}必须是合法 URL` };
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return { ok: false, message: `${label}仅支持 http/https` };
  }
  if (parsed.host.length === 0) return { ok: false, message: `${label}缺少主机名` };
  return { ok: true, message: "" };
}

/** 端口范围校验(1—65535;空由调用方决定是否必填)。 */
export function validatePort(
  raw: string,
  label = "端口"
): { ok: boolean; value: number; message: string } {
  const t = raw.trim();
  if (!/^\d+$/.test(t)) return { ok: false, value: 0, message: `${label}应为 1—65535 的整数` };
  const v = Number(t);
  if (v < 1 || v > 65535) return { ok: false, value: 0, message: `${label}范围 1—65535` };
  return { ok: true, value: v, message: "" };
}

/** telegram chat_id 形状(后端口径:数字 / @频道名 / - 开头群组)。 */
export function validateChatId(raw: string): { ok: boolean; message: string } {
  const t = raw.trim();
  if (t.length === 0) return { ok: false, message: "chat_id 不能为空" };
  if (/^-?\d+$/.test(t) || t.startsWith("@") || t.startsWith("-")) {
    return { ok: true, message: "" };
  }
  return { ok: false, message: "chat_id 应为数字、@频道名或 -100 开头群组" };
}

/** 邮箱形状(后端口径:包含 @;比 RFC 校验宽松,以后端为准)。 */
export function validateEmailAddress(raw: string, label: string): { ok: boolean; message: string } {
  const t = raw.trim();
  if (t.length === 0) return { ok: false, message: `${label}不能为空` };
  if (!t.includes("@")) return { ok: false, message: `${label}不是合法邮箱地址` };
  return { ok: true, message: "" };
}

// ===== 渠道表单 → 请求载荷(POST/PUT 共用)=====

export interface ChannelFormInput {
  kind: ChannelKind;
  name: string;
  enabled: boolean;
  /** webhook/dingtalk/feishu/wecom 的回调/机器人地址(config.url)。 */
  url: string;
  /** dingtalk/feishu 可选加签秘密(secrets.secret;留空=不修改/不设置)。 */
  secret: string;
  /** bark 服务端(config.server_url)。 */
  serverUrl: string;
  /** bark 设备号(secrets.device_key;创建必填)。 */
  deviceKey: string;
  /** telegram 机器人 token(secrets.bot_token;创建必填)。 */
  botToken: string;
  /** telegram 会话(config.chat_id)。 */
  chatId: string;
  /** email 收件邮箱(config.to_address)。 */
  toAddress: string;
  /** email 复用系统 SMTP(config.use_system_smtp)。 */
  useSystemSmtp: boolean;
  /** email 独立 SMTP(config.smtp_host/smtp_port/from_address)。 */
  smtpHost: string;
  smtpPort: string;
  /** email 独立 SMTP 凭据(secrets.smtp_user/smtp_password;可选)。 */
  smtpUser: string;
  smtpPassword: string;
  fromAddress: string;
  eventTypes: EventType[];
}

export interface ChannelWritePayload {
  kind: ChannelKind;
  name: string;
  enabled: boolean;
  config: Record<string, unknown>;
  /** 只含非空值:空包 = 创建无秘密 / 编辑不修改(整包替换语义见后端)。 */
  secrets: Record<string, string>;
  event_types: string[];
}

/**
 * 渠道表单 → POST/PUT 载荷;非法时逐项中文说明拼接。
 * secretsRequired=true(创建或整包替换)时必填秘密键(bark/telegram)不可为空。
 */
export function buildChannelPayload(
  f: ChannelFormInput,
  opts: { secretsRequired: boolean }
): { ok: true; payload: ChannelWritePayload } | { ok: false; error: string } {
  const violations: string[] = [];
  const name = validateChannelName(f.name);
  if (!name.ok) violations.push(name.message);

  const config: Record<string, unknown> = {};
  const secrets: Record<string, string> = {};
  const putSecret = (key: string, value: string): void => {
    const v = value.trim();
    if (v.length > 0) secrets[key] = v;
  };

  switch (f.kind) {
    case "webhook":
    case "dingtalk":
    case "feishu":
    case "wecom": {
      const url = validateHttpUrl(f.url, "Webhook 地址");
      if (!url.ok) violations.push(url.message);
      else config["url"] = f.url.trim();
      if (f.kind === "dingtalk" || f.kind === "feishu") putSecret("secret", f.secret);
      break;
    }
    case "bark": {
      const server = validateHttpUrl(f.serverUrl, "服务器地址");
      if (!server.ok) violations.push(server.message);
      else config["server_url"] = f.serverUrl.trim();
      putSecret("device_key", f.deviceKey);
      if (opts.secretsRequired && secrets["device_key"] === undefined) {
        violations.push("bark 渠道必填秘密字段 device_key");
      }
      break;
    }
    case "telegram": {
      const chat = validateChatId(f.chatId);
      if (!chat.ok) violations.push(chat.message);
      else config["chat_id"] = f.chatId.trim();
      putSecret("bot_token", f.botToken);
      if (opts.secretsRequired && secrets["bot_token"] === undefined) {
        violations.push("telegram 渠道必填秘密字段 bot_token");
      }
      break;
    }
    case "email": {
      const to = validateEmailAddress(f.toAddress, "收件邮箱");
      if (!to.ok) violations.push(to.message);
      else config["to_address"] = f.toAddress.trim();
      config["use_system_smtp"] = f.useSystemSmtp;
      if (!f.useSystemSmtp) {
        const host = f.smtpHost.trim();
        if (host.length === 0) violations.push("未启用系统 SMTP 时必填 SMTP 服务器");
        else config["smtp_host"] = host;
        const portRaw = f.smtpPort.trim();
        if (portRaw.length > 0) {
          const port = validatePort(portRaw, "SMTP 端口");
          if (!port.ok) violations.push(port.message);
          else config["smtp_port"] = port.value;
        }
        const from = validateEmailAddress(f.fromAddress, "发件地址");
        if (!from.ok) violations.push(from.message);
        else config["from_address"] = f.fromAddress.trim();
      }
      putSecret("smtp_user", f.smtpUser);
      putSecret("smtp_password", f.smtpPassword);
      break;
    }
  }

  if (violations.length > 0) return { ok: false, error: violations.join(";") };
  return {
    ok: true,
    payload: {
      kind: f.kind,
      name: f.name.trim(),
      enabled: f.enabled,
      config,
      secrets,
      event_types: f.eventTypes.slice()
    }
  };
}

// ===== 系统 SMTP 表单 → PUT 载荷(密码留空=不修改)=====

export interface SmtpFormInput {
  host: string;
  port: string;
  encryption: string;
  username: string;
  /** 留空 = 不修改已保存密码(脱敏编辑)。 */
  password: string;
  fromName: string;
  fromAddress: string;
}

export interface SmtpWritePayload {
  host: string;
  port: number;
  encryption: SmtpEncryption;
  from_address: string;
  from_name: string;
  username: string;
  /** 仅在填写新密码时携带该键。 */
  password?: string;
}

export function buildSystemSmtpPayload(
  f: SmtpFormInput
): { ok: true; payload: SmtpWritePayload } | { ok: false; error: string } {
  const violations: string[] = [];
  const host = f.host.trim();
  if (host.length === 0) violations.push("SMTP 服务器不能为空");
  const port = validatePort(f.port, "SMTP 端口");
  if (!port.ok) violations.push(port.message);
  let encryption: SmtpEncryption | null = null;
  if (f.encryption === "none" || f.encryption === "starttls" || f.encryption === "ssl") {
    encryption = f.encryption;
  } else {
    violations.push("加密方式仅支持 none/starttls/ssl");
  }
  const from = validateEmailAddress(f.fromAddress, "发件地址");
  if (!from.ok) violations.push(from.message);
  if (violations.length > 0 || encryption === null) {
    return { ok: false, error: violations.join(";") };
  }
  const payload: SmtpWritePayload = {
    host,
    port: port.value,
    encryption,
    from_address: f.fromAddress.trim(),
    from_name: f.fromName.trim(),
    username: f.username.trim()
  };
  const password = f.password.trim();
  if (password.length > 0) payload.password = password;
  return { ok: true, payload };
}

// ===== "如何获取配置"指引(编辑器折叠区,按类型展示)=====

export const KIND_GUIDES: Record<ChannelKind, string> = {
  webhook:
    "准备一个能接收 POST 请求的 http(s) 地址(自建服务或第三方集成平台的 Webhook 入口)。" +
    "通知以 JSON POST 发送,字段:event、title、content、timestamp。",
  dingtalk:
    "钉钉群 → 群设置 → 机器人 → 添加「自定义」机器人 → 安全设置任选一种 → 复制 Webhook 地址填入。" +
    "若选择「加签」,把 SEC 开头的密钥填入下方加签 secret(未开启加签则留空)。",
  feishu:
    "飞书群 → 设置 → 群机器人 → 添加「Custom Bot」→ 复制 Webhook 地址填入。" +
    "若开启「签名校验」,把密钥填入下方签名 secret(未开启则留空)。",
  wecom: "企业微信群 → 右键群聊 → 添加群机器人 → 复制新机器人的 Webhook 地址填入。",
  bark:
    "在 iPhone 上安装 Bark App,打开后复制页面中的设备 Key 填入 device_key。" +
    "默认服务端为 https://api.day.app;自建 Bark 服务端时可修改服务器地址。",
  telegram:
    "与 @BotFather 对话,发送 /newbot 按提示创建机器人,复制 Token(形如 123456789:AA…)填入 bot_token。" +
    "chat_id 可向 @userinfobot 发消息查询(个人为数字);频道用 @名称,群组为 -100 开头的数字。",
  email:
    "填写收件邮箱。默认勾选「使用系统 SMTP」,由本页上方「系统 SMTP」统一发信(需先配置)。" +
    "渠道需独立邮箱服务时取消勾选,填写 SMTP 服务器/端口/加密方式与发件地址;" +
    "账号/密码为可选(匿名中继可留空)。"
};

/** 设置页(系统与AI)视图模型(007 T079/T080,FR-070/FR-072):
 *  系统设置契约解析(ai_key_configured 脱敏派生)、AI 配置表单校验与载荷构建
 *  (地址 http(s) 形状/Key 留空=不修改)、模型列表与测试连接结果解析(缺失降级)、
 *  管理员凭据表单校验(当前密码必填/新密码≥12/两次一致/至少一项变更)。
 *  校验口径与 src/application/settings_sys.rs、src/application/auth.rs 对齐(宪章 II);
 *  "算"与"摆"分离:本文件不触 DOM,页面(page.ts)只装配。 */
import { isRecord } from "../../shared/contracts";

// ===== 系统设置契约(GET/PUT /settings/system;Key 只回 *_configured)=====

export interface SystemSettingsDto {
  aiApiUrl: string;
  aiModel: string;
  aiKeyConfigured: boolean;
}

export function parseSystemSettings(v: unknown): SystemSettingsDto {
  if (!isRecord(v)) throw new Error("系统设置响应格式错误");
  return {
    aiApiUrl: String(v["ai_api_url"] ?? ""),
    aiModel: String(v["ai_model"] ?? ""),
    aiKeyConfigured: v["ai_key_configured"] === true
  };
}

/** API Key 占位派生:已配置时绝不回显明文,留空保存=不修改(FR-070)。 */
export function apiKeyPlaceholder(keyConfigured: boolean): string {
  return keyConfigured ? "已配置(留空不修改)" : "未设置";
}

// ===== AI 配置表单(与 settings_sys.rs 同口径)=====

/** 模型名长度上限(MODEL_MAX_CHARS)。 */
export const AI_MODEL_MAX_CHARS = 200;
/** API Key 长度上限(API_KEY_MAX_CHARS)。 */
export const AI_API_KEY_MAX_CHARS = 4096;

/** 地址形状校验(后端 put_system 同口径:非空时必须 http(s) 前缀;空串=清除配置)。 */
export function validateAiApiUrl(raw: string): { ok: boolean; message: string } {
  const t = raw.trim();
  if (t.length === 0) return { ok: true, message: "" };
  if (!t.startsWith("http://") && !t.startsWith("https://")) {
    return { ok: false, message: "AI 服务地址必须以 http:// 或 https:// 开头" };
  }
  return { ok: true, message: "" };
}

export interface AiFormInput {
  apiUrl: string;
  model: string;
  /** 留空 = 不修改已保存 Key(脱敏编辑)。 */
  apiKey: string;
}

export interface AiWritePayload {
  /** 空串 = 清除(后端 Some("") 语义)。 */
  ai_api_url: string;
  ai_model: string;
  /** 仅在填写新 Key 时携带该键(留空=不修改)。 */
  ai_api_key?: string;
}

export function buildAiPayload(
  f: AiFormInput
): { ok: true; payload: AiWritePayload } | { ok: false; error: string } {
  const violations: string[] = [];
  const url = validateAiApiUrl(f.apiUrl);
  if (!url.ok) violations.push(url.message);
  const model = f.model.trim();
  if ([...model].length > AI_MODEL_MAX_CHARS) violations.push(`模型名不能超过 ${AI_MODEL_MAX_CHARS} 字`);
  const key = f.apiKey.trim();
  if ([...key].length > AI_API_KEY_MAX_CHARS) violations.push(`API Key 不能超过 ${AI_API_KEY_MAX_CHARS} 字`);
  if (violations.length > 0) return { ok: false, error: violations.join(";") };
  const payload: AiWritePayload = { ai_api_url: f.apiUrl.trim(), ai_model: model };
  if (key.length > 0) payload.ai_api_key = key;
  return { ok: true, payload };
}

// ===== 模型列表 / 测试连接结果(缺失降级,不冒充真实值)=====

/** {models:[…]} → 候选列表;字段缺失/形状不对降级为空数组(界面显示"0 个"如实)。 */
export function parseAiModels(v: unknown): string[] {
  if (!isRecord(v) || !Array.isArray(v["models"])) return [];
  return v["models"]
    .map((m) => String(m).trim())
    .filter((m) => m.length > 0);
}

export interface AiTestReport {
  model: string;
  latencyMs: number | null;
  reply: string;
}

/** {model, latency_ms, reply} → 缺失字段降级(模型 ""/延迟 null/回复 ""),由摘要层标注。 */
export function parseAiTestReport(v: unknown): AiTestReport {
  if (!isRecord(v)) throw new Error("AI 测试结果格式错误");
  const latency = v["latency_ms"];
  return {
    model: typeof v["model"] === "string" ? v["model"] : "",
    latencyMs: typeof latency === "number" ? latency : null,
    reply: typeof v["reply"] === "string" ? v["reply"] : ""
  };
}

/** 回复摘要截断(超长截断 + 省略号;单行化避免结果行被换行撑爆)。 */
export function summarizeReply(reply: string, max = 80): string {
  const oneLine = reply.trim().replace(/\s+/g, " ");
  return oneLine.length > max ? `${oneLine.slice(0, max)}…` : oneLine;
}

/** 测试结果行文案:"✓ 模型 X · 123ms · 回复摘要"(缺失项显示 —,不虚构)。 */
export function aiTestSummary(r: AiTestReport): string {
  const model = r.model.length > 0 ? r.model : "—";
  const latency = r.latencyMs !== null ? `${r.latencyMs}ms` : "—";
  const reply = summarizeReply(r.reply);
  return `模型 ${model} · ${latency}${reply.length > 0 ? ` · 回复:${reply}` : " · 无回复内容"}`;
}

// ===== 管理员凭据表单(FR-072;与 auth.rs change_credentials 同口径)=====

export const MIN_PASSWORD_CHARS = 12;

export interface CredentialsFormInput {
  newUsername: string;
  currentPassword: string;
  /** 留空 = 不修改。 */
  newPassword: string;
  confirmPassword: string;
}

export interface CredentialsWritePayload {
  current_password: string;
  /** 仅在填写新用户名时携带(留空=不修改)。 */
  new_username?: string;
  /** 仅在填写新密码时携带(留空=不修改)。 */
  new_password?: string;
}

export function buildCredentialsPayload(
  f: CredentialsFormInput
): { ok: true; payload: CredentialsWritePayload } | { ok: false; error: string } {
  const violations: string[] = [];
  const current = f.currentPassword.trim();
  if (current.length === 0) violations.push("必须填写当前密码");
  const username = f.newUsername.trim();
  const password = f.newPassword.trim();
  const confirm = f.confirmPassword.trim();
  if (username.length === 0 && password.length === 0) {
    violations.push("至少填写新用户名或新密码之一");
  }
  if (password.length > 0 && [...password].length < MIN_PASSWORD_CHARS) {
    violations.push(`新密码至少 ${MIN_PASSWORD_CHARS} 个字符`);
  }
  if (confirm !== password) violations.push("两次输入的新密码不一致");
  if (violations.length > 0) return { ok: false, error: violations.join(";") };
  const payload: CredentialsWritePayload = { current_password: current };
  if (username.length > 0) payload.new_username = username;
  if (password.length > 0) payload.new_password = password;
  return { ok: true, payload };
}

// ===== 账号 AI 提示词(FR-071;transport AI_PROMPT_MAX_CHARS 同口径)=====

export const AI_PROMPT_MAX_CHARS = 2000;

/** 提示词长度校验(unicode 字符计数,与后端 chars().count() 同口径;空=合法)。 */
export function validateAiPrompt(raw: string): { ok: boolean; message: string } {
  if ([...raw].length > AI_PROMPT_MAX_CHARS) {
    return { ok: false, message: `提示词不能超过 ${AI_PROMPT_MAX_CHARS} 字` };
  }
  return { ok: true, message: "" };
}

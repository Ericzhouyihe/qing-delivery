/** 设置页(系统与AI)视图模型测试(007 T080):
 *  系统设置契约解析与 ai_key_configured 占位派生、AI 配置表单校验与载荷构建
 *  (url 形状/Key 留空=不修改)、模型列表与测试连接结果解析(缺失降级)、
 *  管理员凭据表单校验(当前密码必填/新密码≥12/两次一致/至少一项变更)、
 *  账号 AI 提示词长度校验——与 settings_sys.rs、auth.rs、settings_api.rs 同口径。 */
import { describe, expect, it } from "vitest";
import {
  AI_MODEL_MAX_CHARS,
  AI_PROMPT_MAX_CHARS,
  MIN_PASSWORD_CHARS,
  aiTestSummary,
  apiKeyPlaceholder,
  buildAiPayload,
  buildCredentialsPayload,
  parseAiModels,
  parseAiTestReport,
  parseSystemSettings,
  summarizeReply,
  validateAiApiUrl,
  validateAiPrompt,
  type AiFormInput,
  type CredentialsFormInput
} from "./model";

const aiForm = (over: Partial<AiFormInput> = {}): AiFormInput => ({
  apiUrl: "https://ai.example.com/v1",
  model: "gpt-test",
  apiKey: "",
  ...over
});

const credForm = (over: Partial<CredentialsFormInput> = {}): CredentialsFormInput => ({
  newUsername: "",
  currentPassword: "current-pass-123",
  newPassword: "a-long-password-456",
  confirmPassword: "a-long-password-456",
  ...over
});

describe("parseSystemSettings 契约解析(GET /settings/system)", () => {
  it("完整解析(Key 只回布尔,永不携带明文)", () => {
    const v = parseSystemSettings({
      ai_api_url: "https://ai.example.com/v1",
      ai_model: "gpt-test",
      ai_key_configured: true
    });
    expect(v.aiApiUrl).toBe("https://ai.example.com/v1");
    expect(v.aiModel).toBe("gpt-test");
    expect(v.aiKeyConfigured).toBe(true);
  });

  it("字段缺失降级:空地址/空模型/未配置 Key 不冒充真实值", () => {
    const v = parseSystemSettings({});
    expect(v).toEqual({ aiApiUrl: "", aiModel: "", aiKeyConfigured: false });
  });

  it("非对象抛错(契约边界)", () => {
    expect(() => parseSystemSettings(null)).toThrow();
    expect(() => parseSystemSettings("x")).toThrow();
    expect(() => parseSystemSettings([])).toThrow();
  });
});

describe("apiKeyPlaceholder 占位派生(FR-070 脱敏回显)", () => {
  it("已配置 → 「已配置(留空不修改)」;未配置 → 「未设置」", () => {
    expect(apiKeyPlaceholder(true)).toBe("已配置(留空不修改)");
    expect(apiKeyPlaceholder(false)).toBe("未设置");
  });
});

describe("validateAiApiUrl 地址形状(settings_sys.rs 同口径)", () => {
  it("空串合法(=清除 AI 配置)", () => {
    expect(validateAiApiUrl("").ok).toBe(true);
    expect(validateAiApiUrl("   ").ok).toBe(true);
  });

  it("http/https 前缀合法;其余协议/裸文本拒绝", () => {
    expect(validateAiApiUrl("https://api.openai.com/v1").ok).toBe(true);
    expect(validateAiApiUrl("http://127.0.0.1:11434/v1").ok).toBe(true);
    expect(validateAiApiUrl("ftp://x").ok).toBe(false);
    expect(validateAiApiUrl("api.openai.com/v1").ok).toBe(false);
  });
});

describe("buildAiPayload 载荷构建(PUT /settings/system)", () => {
  it("Key 留空 → 不携带 ai_api_key 键(=不修改已保存 Key)", () => {
    const built = buildAiPayload(aiForm());
    expect(built.ok).toBe(true);
    if (!built.ok) return;
    expect(built.payload).toEqual({
      ai_api_url: "https://ai.example.com/v1",
      ai_model: "gpt-test"
    });
    expect("ai_api_key" in built.payload).toBe(false);
  });

  it("Key 非空 → 携带 trim 后的 ai_api_key(=信封重封)", () => {
    const built = buildAiPayload(aiForm({ apiKey: "  sk-plain-1  " }));
    expect(built.ok).toBe(true);
    if (!built.ok) return;
    expect(built.payload.ai_api_key).toBe("sk-plain-1");
  });

  it("地址/模型 trim;地址空串原样携带(=清除)", () => {
    const built = buildAiPayload(aiForm({ apiUrl: "  ", model: "  m-1  " }));
    expect(built.ok).toBe(true);
    if (!built.ok) return;
    expect(built.payload.ai_api_url).toBe("");
    expect(built.payload.ai_model).toBe("m-1");
  });

  it("非法地址/超长模型/超长 Key → 逐项错误,不产出载荷", () => {
    expect(buildAiPayload(aiForm({ apiUrl: "ftp://x" })).ok).toBe(false);
    expect(buildAiPayload(aiForm({ model: "m".repeat(AI_MODEL_MAX_CHARS + 1) })).ok).toBe(false);
    expect(buildAiPayload(aiForm({ apiKey: "k".repeat(4097) })).ok).toBe(false);
    const both = buildAiPayload(aiForm({ apiUrl: "x", model: "m".repeat(AI_MODEL_MAX_CHARS + 1) }));
    if (both.ok) throw new Error("应当失败");
    expect(both.error).toContain("http");
    expect(both.error).toContain("模型名");
  });
});

describe("parseAiModels 模型列表解析(POST /settings/ai/models,缺失降级)", () => {
  it("完整列表解析", () => {
    expect(parseAiModels({ models: ["gpt-4o-mini", "deepseek-chat", "qwen-max"] })).toEqual([
      "gpt-4o-mini",
      "deepseek-chat",
      "qwen-max"
    ]);
  });

  it("models 缺失/非数组/非对象 → 降级空数组(界面按 0 个如实展示)", () => {
    expect(parseAiModels({})).toEqual([]);
    expect(parseAiModels({ models: "gpt-4o" })).toEqual([]);
    expect(parseAiModels(null)).toEqual([]);
  });

  it("空串项被过滤(不产出空候选)", () => {
    expect(parseAiModels({ models: ["gpt-4o", "", "  "] })).toEqual(["gpt-4o"]);
  });
});

describe("parseAiTestReport / aiTestSummary 测试连接结果(POST /settings/ai/test)", () => {
  it("完整解析:模型/耗时/回复", () => {
    const r = parseAiTestReport({ model: "gpt-test", latency_ms: 123, reply: "你好,我是小店客服" });
    expect(r).toEqual({ model: "gpt-test", latencyMs: 123, reply: "你好,我是小店客服" });
    expect(aiTestSummary(r)).toBe("模型 gpt-test · 123ms · 回复:你好,我是小店客服");
  });

  it("字段缺失降级:模型 — /延迟 — /无回复(不虚构)", () => {
    const r = parseAiTestReport({});
    expect(r.model).toBe("");
    expect(r.latencyMs).toBeNull();
    expect(r.reply).toBe("");
    expect(aiTestSummary(r)).toBe("模型 — · — · 无回复内容");
  });

  it("latency_ms 非数字降级 null;非对象抛错", () => {
    expect(parseAiTestReport({ model: "m", latency_ms: "123" }).latencyMs).toBeNull();
    expect(() => parseAiTestReport("x")).toThrow();
  });

  it("回复摘要:多行压单行 + 超长截断", () => {
    expect(summarizeReply("第一行\n第二行  继续")).toBe("第一行 第二行 继续");
    const long = "字".repeat(100);
    expect(summarizeReply(long, 80)).toBe(`${"字".repeat(80)}…`);
    expect(summarizeReply(long, 80).length).toBe(81);
  });
});

describe("buildCredentialsPayload 凭据表单(FR-072,auth.rs 同口径)", () => {
  it("合法:密码变更(留空用户名不携带 new_username)", () => {
    const built = buildCredentialsPayload(credForm());
    expect(built.ok).toBe(true);
    if (!built.ok) return;
    expect(built.payload).toEqual({
      current_password: "current-pass-123",
      new_password: "a-long-password-456"
    });
    expect("new_username" in built.payload).toBe(false);
  });

  it("合法:仅改用户名(留空密码不携带 new_password)", () => {
    const built = buildCredentialsPayload(
      credForm({ newUsername: "shop-admin", newPassword: "", confirmPassword: "" })
    );
    expect(built.ok).toBe(true);
    if (!built.ok) return;
    expect(built.payload).toEqual({ current_password: "current-pass-123", new_username: "shop-admin" });
  });

  it("当前密码必填", () => {
    const built = buildCredentialsPayload(credForm({ currentPassword: "  " }));
    expect(built.ok).toBe(false);
    if (built.ok) return;
    expect(built.error).toContain("当前密码");
  });

  it("新密码至少 12 个字符(MIN_PASSWORD_CHARS)", () => {
    const built = buildCredentialsPayload(credForm({ newPassword: "short", confirmPassword: "short" }));
    expect(built.ok).toBe(false);
    if (built.ok) return;
    expect(built.error).toContain(`${MIN_PASSWORD_CHARS}`);
    // 恰好 12 字合法
    const exact = "x".repeat(MIN_PASSWORD_CHARS);
    expect(buildCredentialsPayload(credForm({ newPassword: exact, confirmPassword: exact })).ok).toBe(true);
  });

  it("两次输入的新密码必须一致", () => {
    const built = buildCredentialsPayload(credForm({ confirmPassword: "different-long-1" }));
    expect(built.ok).toBe(false);
    if (built.ok) return;
    expect(built.error).toContain("不一致");
  });

  it("用户名与新密码全留空 → 拒绝(至少一项变更)", () => {
    const built = buildCredentialsPayload(
      credForm({ newUsername: "", newPassword: "", confirmPassword: "" })
    );
    expect(built.ok).toBe(false);
    if (built.ok) return;
    expect(built.error).toContain("至少");
  });

  it("多项违规逐条拼接", () => {
    const built = buildCredentialsPayload(
      credForm({ currentPassword: "", newPassword: "short", confirmPassword: "other" })
    );
    expect(built.ok).toBe(false);
    if (built.ok) return;
    expect(built.error.split(";").length).toBe(3);
  });
});

describe("validateAiPrompt 账号提示词(FR-071,settings_api.rs 同口径)", () => {
  it("空/上限内合法;超限拒绝(unicode 字符计数)", () => {
    expect(validateAiPrompt("").ok).toBe(true);
    expect(validateAiPrompt("你是小店客服").ok).toBe(true);
    expect(validateAiPrompt("字".repeat(AI_PROMPT_MAX_CHARS)).ok).toBe(true);
    const over = validateAiPrompt("字".repeat(AI_PROMPT_MAX_CHARS + 1));
    expect(over.ok).toBe(false);
    expect(over.message).toContain(`${AI_PROMPT_MAX_CHARS}`);
  });
});

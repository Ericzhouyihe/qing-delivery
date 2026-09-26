/** 设置页(系统与AI,007 T079 扩展):运行信息/安全/备份恢复/关于只读卡零回退(FR-073),
 *  新增 AI 智能回复配置卡(FR-070:地址/Key 脱敏/模型候选/测试连接)与
 *  管理员凭据卡(FR-072:openConfirmFlow,改密后其他会话全部退出)。
 *  "算"在 model.ts(纯函数);本文件只装配与调用既有 API(宪章 II)。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpGet, httpPost, httpPut } from "../../shared/http";
import { errorHint, isRecord } from "../../shared/contracts";
import { btn, muted } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { confirmDialog } from "../../ui/modal";
import { formatRelative } from "../../ui/time";
import { bootstrapSession, session } from "../../app/session";
import {
  aiTestSummary,
  apiKeyPlaceholder,
  buildAiPayload,
  buildCredentialsPayload,
  parseAiModels,
  parseAiTestReport,
  parseSystemSettings,
  type SystemSettingsDto
} from "./model";
import type { PageFactory } from "../../app/page";

interface Capabilities {
  executionProfile: string;
  platforms: Array<{ platform: string; supported: boolean; reason?: string }>;
  contentLimits: { unicodeScalars: number; utf8Bytes: number };
}

function parseCapabilities(v: unknown): Capabilities {
  if (!isRecord(v)) throw new Error("能力响应格式错误");
  const platformsRaw = Array.isArray(v["platforms"]) ? v["platforms"] : [];
  const limits = (v["content_limits"] ?? {}) as Record<string, unknown>;
  return {
    executionProfile: String(v["execution_profile"] ?? "live"),
    platforms: platformsRaw.map((p) => {
      const r = p as Record<string, unknown>;
      return {
        platform: String(r["platform"] ?? ""),
        supported: r["supported"] === true,
        reason: typeof r["reason"] === "string" ? r["reason"] : undefined
      };
    }),
    contentLimits: {
      unicodeScalars: typeof limits["unicode_scalars"] === "number" ? limits["unicode_scalars"] : 0,
      utf8Bytes: typeof limits["utf8_bytes"] === "number" ? limits["utf8_bytes"] : 0
    }
  };
}

/** 统一错误文案:已知码走 errorHint,未知码/无信封用原始 message(AI 上游原因即在其中)。 */
const apiErrorMessage = (e: unknown): string => {
  if (e instanceof ApiHttpError) return errorHint(e.body?.code, e.message);
  return e instanceof Error ? e.message : String(e);
};

export const settingsPage: PageFactory = (root) => {
  const stop = asyncBlock(root, async () => {
    const [capsRaw, restoreRaw, systemRaw] = await Promise.all([
      httpGet("/api/v1/capabilities"),
      httpGet("/api/v1/restore").catch(() => null),
      // 旧服务无该端点:表单按"未配置"呈现并如实标注加载失败,不阻断页面
      httpGet("/api/v1/settings/system").catch(() => null)
    ]);
    const caps = parseCapabilities(capsRaw);
    const restore = isRecord(restoreRaw) ? restoreRaw : null;
    const page = el("div", { class: "settings-grid" });

    // 运行信息卡(T050,零回退)
    const profileLabel = caps.executionProfile === "mock" ? "mock(开发模拟)" : "live(真实平台)";
    page.append(
      el(
        "section",
        { class: "card" },
        el("h2", { class: "card-title" }, "运行信息"),
        el("dl", { class: "kv" },
          el("div", {}, el("dt", {}, "执行模式"), el("dd", {}, profileLabel)),
          el("div", {}, el("dt", {}, "运行方式"), el("dd", {}, "本机服务(关闭本页面后继续运行)")),
          el("div", {}, el("dt", {}, "内容上限"), el("dd", {}, `${caps.contentLimits.unicodeScalars} 字符 / ${caps.contentLimits.utf8Bytes} 字节`))
        ),
        el(
          "div",
          { class: "badge-row", style: "margin-top:8px;" },
          ...caps.platforms.map((p) =>
            el(
              "span",
              {
                class: p.supported ? "badge badge-normal" : "badge badge-neutral",
                title: p.reason ?? ""
              },
              `${p.platform}${p.supported ? " · 支持" : " · 不支持"}`
            )
          )
        )
      )
    );

    // ---- AI 智能回复配置卡(007 T079,FR-070)----
    const aiUrlInput = el("input", {
      type: "text",
      placeholder: "https://api.openai.com/v1(OpenAI 兼容服务根地址)",
      autocomplete: "off"
    }) as HTMLInputElement;
    const aiModelInput = el("input", {
      type: "text",
      list: "sys-ai-model-candidates",
      placeholder: "如 gpt-4o-mini,可点「读取模型」拉取候选",
      autocomplete: "off"
    }) as HTMLInputElement;
    const modelCandidates = el("datalist", { id: "sys-ai-model-candidates" });
    const aiKeyInput = el("input", {
      type: "password",
      autocomplete: "new-password"
    }) as HTMLInputElement;
    const aiKeyHint = el("div", { class: "hint" }, "");
    const modelsBtn = btn("读取模型", { variant: "secondary" });
    const testBtn = btn("测试连接", { variant: "secondary" });
    const saveAiBtn = btn("保存 AI 配置", { variant: "primary" });
    const modelsLine = el("div", { class: "sysai-result-line", style: "display:none;" }, "");
    const testLine = el("div", { class: "sysai-result-line", style: "display:none;" }, "");
    const aiError = el("div", { class: "field-error" }, "");
    const aiSavedLine = el("div", { class: "sysai-result-line", style: "display:none;" }, "");

    const fillAiForm = (dto: SystemSettingsDto): void => {
      aiUrlInput.value = dto.aiApiUrl;
      aiModelInput.value = dto.aiModel;
      aiKeyInput.value = "";
      aiKeyInput.placeholder = apiKeyPlaceholder(dto.aiKeyConfigured);
      aiKeyHint.textContent = dto.aiKeyConfigured
        ? "Key 已在本机加密存储,界面不回显明文;留空保存=不修改"
        : "尚未配置 Key;填写后保存(经本机加密存储,界面永不回显)";
    };

    if (systemRaw !== null) {
      fillAiForm(parseSystemSettings(systemRaw));
    } else {
      fillAiForm({ aiApiUrl: "", aiModel: "", aiKeyConfigured: false });
      aiError.textContent = "AI 配置加载失败:无法读取系统设置(可填写后保存覆盖)";
    }

    modelsBtn.addEventListener("click", async () => {
      modelsBtn.disabled = true;
      const label = modelsBtn.textContent;
      modelsBtn.textContent = "读取中…";
      modelsLine.style.display = "block";
      modelsLine.replaceChildren(el("span", { class: "muted" }, "正在向 AI 服务请求模型列表…"));
      try {
        // base_url 传当前输入(未保存的地址试探);空串由后端回退到已存地址
        const models = parseAiModels(
          await httpPost("/api/v1/settings/ai/models", { base_url: aiUrlInput.value.trim() })
        );
        modelCandidates.replaceChildren(
          ...models.map((m) => el("option", { value: m }) as HTMLOptionElement)
        );
        modelsLine.replaceChildren(
          el(
            "span",
            { class: models.length > 0 ? "tone-normal" : "error-text" },
            models.length > 0
              ? `✓ 获取到 ${models.length} 个候选,点击模型输入框选择`
              : "✗ 上游未返回任何模型,可手动填写模型名"
          )
        );
      } catch (e) {
        modelsLine.replaceChildren(
          el("span", { class: "error-text" }, `✗ 读取模型失败:${apiErrorMessage(e)}`)
        );
      } finally {
        modelsBtn.disabled = false;
        modelsBtn.textContent = label;
      }
    });

    testBtn.addEventListener("click", async () => {
      testBtn.disabled = true;
      const label = testBtn.textContent;
      testBtn.textContent = "测试中…";
      testLine.style.display = "block";
      testLine.replaceChildren(el("span", { class: "muted" }, "正在发起最小测试对话…"));
      try {
        const report = parseAiTestReport(
          await httpPost("/api/v1/settings/ai/test", { base_url: aiUrlInput.value.trim() })
        );
        testLine.replaceChildren(el("span", { class: "tone-normal" }, `✓ ${aiTestSummary(report)}`));
      } catch (e) {
        testLine.replaceChildren(
          el("span", { class: "error-text" }, `✗ 测试失败:${apiErrorMessage(e)}`)
        );
      } finally {
        testBtn.disabled = false;
        testBtn.textContent = label;
      }
    });

    saveAiBtn.addEventListener("click", async () => {
      const built = buildAiPayload({
        apiUrl: aiUrlInput.value,
        model: aiModelInput.value,
        apiKey: aiKeyInput.value
      });
      if (!built.ok) {
        aiError.textContent = built.error;
        aiSavedLine.style.display = "none";
        return;
      }
      saveAiBtn.disabled = true;
      try {
        fillAiForm(parseSystemSettings(await httpPut("/api/v1/settings/system", built.payload)));
        aiError.textContent = "";
        aiSavedLine.style.display = "block";
        aiSavedLine.replaceChildren(el("span", { class: "tone-normal" }, "✓ AI 配置已保存"));
      } catch (e) {
        aiError.textContent = `保存失败:${apiErrorMessage(e)}`;
        aiSavedLine.style.display = "none";
      } finally {
        saveAiBtn.disabled = false;
      }
    });

    page.append(
      el(
        "section",
        { class: "card" },
        el("h2", { class: "card-title" }, "AI 智能回复配置"),
        el(
          "p",
          { class: "muted", style: "margin:0 0 12px;font-size:var(--text-xs);" },
          "配置 OpenAI 兼容服务后,可在账号管理中为账号开启 AI 自动回复。AI 仅生成客服回复文案,不参与发货或付款判断(FR-071)。"
        ),
        el(
          "div",
          { class: "sysai-grid" },
          el("div", { class: "field" }, el("label", {}, "API 地址"), aiUrlInput,
            el("div", { class: "hint" }, "服务根地址,以 http:// 或 https:// 开头;清空保存=清除 AI 配置")),
          el("div", { class: "field" }, el("label", {}, "模型"), aiModelInput, modelCandidates,
            el("div", { class: "sysai-inline-action" },
              modelsBtn,
              el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "拉取上游模型列表作为候选"),
              modelsLine))
        ),
        el("div", { class: "field" }, el("label", {}, "API Key"), aiKeyInput, aiKeyHint),
        el("div", { class: "field" },
          el("label", {}, "连通性"),
          el("div", { class: "sysai-inline-action" },
            testBtn,
            el("span", { class: "muted", style: "font-size:var(--text-xs);" },
              "「读取模型/测试连接」使用已保存的 Key 与模型(地址以当前输入覆盖);修改后请先保存"),
            testLine)),
        aiError,
        aiSavedLine,
        el("div", { style: "display:flex;justify-content:flex-end;" }, saveAiBtn)
      )
    );

    // 安全卡(T050,零回退):会话与密码重置指引;管理员名绑定会话态(凭据修改后刷新)
    const adminNameDd = el("dd", {}, session.displayName ?? "admin");
    page.append(
      el(
        "section",
        { class: "card" },
        el("h2", { class: "card-title" }, "安全"),
        el("dl", { class: "kv" },
          el("div", {}, el("dt", {}, "当前管理员"), adminNameDd),
          el("div", {}, el("dt", {}, "会话有效期"), el("dd", {}, "登录后 12 小时(绝对)"))
        ),
        muted("忘记密码:停止服务后运行 qing-delivery.exe init-admin --reset(需本机同用户);重置会使所有会话失效并暂停全部账号。")
      )
    );

    // ---- 管理员凭据卡(007 T079,FR-072:改密后其他会话全部退出)----
    const credUsername = el("input", {
      type: "text",
      placeholder: "留空 = 不修改用户名",
      autocomplete: "off"
    }) as HTMLInputElement;
    const credCurrent = el("input", { type: "password", autocomplete: "current-password" }) as HTMLInputElement;
    const credNew = el("input", {
      type: "password",
      placeholder: "至少 12 个字符;留空 = 不修改密码",
      autocomplete: "new-password"
    }) as HTMLInputElement;
    const credConfirm = el("input", { type: "password", autocomplete: "new-password" }) as HTMLInputElement;
    const credError = el("div", { class: "field-error" }, "");
    const credSavedLine = el("div", { class: "sysai-result-line", style: "display:none;" }, "");
    const credSubmitBtn = btn("修改凭据", { variant: "primary" });

    credSubmitBtn.addEventListener("click", () => {
      const built = buildCredentialsPayload({
        newUsername: credUsername.value,
        currentPassword: credCurrent.value,
        newPassword: credNew.value,
        confirmPassword: credConfirm.value
      });
      if (!built.ok) {
        credError.textContent = built.error;
        credSavedLine.style.display = "none";
        return;
      }
      credError.textContent = "";
      openConfirmFlow({
        title: "修改管理员凭据?",
        impact: el(
          "div",
          {},
          el("p", { style: "margin:0 0 6px;" }, "确认修改后:"),
          el(
            "ul",
            { class: "risk-list" },
            el("li", {}, "修改后其他已登录会话将全部退出,仅保留当前页面"),
            el("li", {}, "请牢记新的用户名与密码;忘记密码需停机运行 init-admin --reset")
          )
        ),
        requireReason: false,
        requireRiskAck: false,
        confirmLabel: "确认修改",
        submit: async () => {
          try {
            await httpPut("/api/v1/auth/credentials", built.payload);
          } catch (e) {
            // 422 credential_change_failed 等已知码走 errorHint 文案(确认流行内呈现)
            throw new Error(apiErrorMessage(e));
          }
        },
        onSuccess: () => {
          credUsername.value = "";
          credCurrent.value = "";
          credNew.value = "";
          credConfirm.value = "";
          credSavedLine.style.display = "block";
          credSavedLine.replaceChildren(
            el("span", { class: "tone-normal" }, "✓ 凭据已更新,其他会话已退出")
          );
          // 刷新会话信息区(重新校验会话并轮换 CSRF;管理员名随会话态更新)
          void bootstrapSession().then(() => {
            adminNameDd.textContent = session.displayName ?? "admin";
          });
        }
      });
    });

    page.append(
      el(
        "section",
        { class: "card" },
        el("h2", { class: "card-title" }, "管理员凭据"),
        el(
          "p",
          { class: "muted", style: "margin:0 0 12px;font-size:var(--text-xs);" },
          "修改用户名或密码需先验证当前密码;两者都留空时不会提交。"
        ),
        el(
          "div",
          { class: "sysai-grid" },
          el("div", { class: "field" }, el("label", {}, "新用户名(留空不改)"), credUsername),
          el("div", { class: "field" }, el("label", {}, "当前密码(必填)"), credCurrent),
          el("div", { class: "field" }, el("label", {}, "新密码(≥12 字符,留空不改)"), credNew),
          el("div", { class: "field" }, el("label", {}, "确认新密码"), credConfirm)
        ),
        credError,
        credSavedLine,
        el("div", { style: "display:flex;justify-content:flex-end;" }, credSubmitBtn)
      )
    );

    // 备份/恢复卡(T051,零回退)
    const backupCard = el("section", { class: "card" });
    backupCard.append(el("h2", { class: "card-title" }, "备份与恢复"));
    if (restore !== null) {
      const started = typeof restore["quarantine_started_at"] === "string" ? restore["quarantine_started_at"] : null;
      backupCard.append(
        el(
          "div",
          { class: "banner banner-warning", role: "status" },
          el("strong", {}, "恢复隔离中"),
          el("span", {}, ` 待核对 ${String(restore["unresolved_count"] ?? "?")} 笔;全部账号已暂停${started !== null ? `,开始于 ${formatRelative(started)}` : ""}。`),
          el("a", { href: "/issues", class: "btn btn-primary btn-sm" }, "去核对 →")
        )
      );
    } else {
      backupCard.append(
        muted("尚无恢复隔离。备份为停机操作:服务运行时无法执行。"),
        el("pre", { class: "preview" }, "qing-delivery.exe backup --output <归档路径>")
      );
    }
    const restoreBtn = btn("查看恢复步骤", { variant: "secondary" });
    restoreBtn.addEventListener("click", () => {
      confirmDialog({
        title: "从备份恢复(停机操作)",
        message: el(
          "div",
          {},
          el("p", {}, "恢复会带来以下后果,请在服务停止后执行:"),
          el("ul", { class: "risk-list" },
            el("li", {}, "进入恢复隔离:快照中未完成的任务全部转为待核对"),
            el("li", {}, "所有管理会话立即吊销,需重新登录"),
            el("li", {}, "全部账号暂停,需逐单核对后手动重新启用")
          ),
          el("pre", { class: "preview" }, "qing-delivery.exe restore --input <归档路径> --data-dir <数据目录>"),
          el("p", { class: "muted" }, "仅支持同机同 Windows 用户恢复;任何失败都会保留原数据。")
        ),
        confirmLabel: "知道了"
      });
    });
    backupCard.append(el("div", { style: "margin-top:8px;" }, restoreBtn));
    page.append(backupCard);

    // 关于卡(T050,零回退):本地运行边界
    page.append(
      el(
        "section",
        { class: "card" },
        el("h2", { class: "card-title" }, "关于"),
        muted("轻交付在本机运行:订单与交付记录保存在本地数据库,凭证经 Windows DPAPI 加密,数据不出本机。"),
        muted("管理页面仅监听回环地址;平台协议变更造成的未知结果会进入「待处理」,绝不盲目重发。")
      )
    );
    return page;
  }, { skeletonLines: 8 });
  return () => {
    stop.dispose();
  };
};

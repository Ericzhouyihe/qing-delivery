/** 通知设置页(007 T064,FR-050~FR-053):页头/工具栏(新建渠道+刷新)/
 *  事件类型说明卡(六种,不选=全部)/系统 SMTP 卡(密码脱敏,留空不修改)/
 *  渠道卡列表(类型图标/徽标/启用开关/订阅摘要/测试投递行内结果/编辑/删除确认流)/
 *  账号绑定区(每账号一行渠道复选组,覆盖式保存;未绑定=全部启用渠道)。
 *  "算"在 model.ts(纯函数);本文件只装配与调用既有 API(宪章 II)。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpGet, httpPost, httpPut, httpRequest } from "../../shared/http";
import { errorHint, isRecord, parsePage } from "../../shared/contracts";
import { badgeEl, type BadgeSpec } from "../../ui/badge";
import { btn } from "../../ui/dom";
import { asyncBlock, type AsyncBlockController } from "../../ui/states";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { confirmDialog } from "../../ui/modal";
import { icon } from "../../ui/icons";
import {
  DEFAULT_SMTP_PORT,
  EVENT_LABELS,
  EVENT_NOTES,
  EVENT_TYPES,
  KIND_ICONS,
  KIND_LABELS,
  KIND_TONES,
  SMTP_ENCRYPTIONS,
  bindingSummary,
  buildSystemSmtpPayload,
  configSummary,
  parseBindings,
  parseChannel,
  parseSystemSmtp,
  parseTestSendResult,
  secretLabel,
  subscriptionSummary,
  testResultText,
  testResultTone,
  uniqueChannelIds,
  type NotificationChannelDto
} from "./model";
import { openChannelEditor } from "./editor";
import type { PageFactory } from "../../app/page";

/** 已知错误码集(与 contracts ERROR_HINTS 收录范围一致)。 */
const ERROR_HINTED = new Set(["referenced_resource"]);

const apiErrorMessage = (e: unknown): string => {
  if (e instanceof ApiHttpError) {
    const code = e.body?.code;
    const hint = code !== undefined && ERROR_HINTED.has(code) ? errorHint(code, e.message) : "";
    return hint.length > 0 ? `${hint}(${e.message})` : e.message;
  }
  return e instanceof Error ? e.message : String(e);
};

interface AccountLite {
  id: string;
  displayName: string;
  connectionState: string;
}

export const notificationsPage: PageFactory = (root) => {
  let channels: NotificationChannelDto[] = [];
  let channelsBlockRef: AsyncBlockController | null = null;
  let bindingsBlockRef: AsyncBlockController | null = null;

  // ---- 页头 + 工具栏(刷新 + 新建渠道)----
  const head = el(
    "div",
    { class: "page-head" },
    el("h1", { class: "page-head-title" }, "通知设置"),
    el(
      "p",
      { class: "page-head-sub" },
      "配置七类通知渠道与事件订阅;账号掉线、交付异常、需要人工处理时第一时间触达。"
    )
  );
  const refreshBtn = btn("刷新", { variant: "secondary" });
  refreshBtn.prepend(icon("refresh"));
  const addBtn = btn("新建渠道", { variant: "primary" });
  addBtn.prepend(icon("edit"));
  const toolbar = el(
    "div",
    { class: "account-toolbar" },
    el(
      "div",
      { class: "filter-bar", style: "margin-bottom:0;" },
      el(
        "span",
        { class: "muted", style: "font-size:var(--text-xs);" },
        "秘密字段全程脱敏:编辑时只显示「已配置」,留空保存表示不修改"
      )
    ),
    el("div", { class: "toolbar-actions" }, refreshBtn, addBtn)
  );
  root.append(head, toolbar);

  // ---- 事件类型说明卡(六种;四类自动化失败归入"需要人工处理")----
  root.append(
    el(
      "section",
      { class: "card" },
      el("h2", { class: "card-title" }, "事件类型"),
      el(
        "dl",
        { class: "notify-event-docs" },
        ...EVENT_TYPES.map((t) =>
          el(
            "div",
            { class: "notify-event-doc" },
            el("dt", {}, EVENT_LABELS[t]),
            el("dd", { class: "muted" }, EVENT_NOTES[t])
          )
        )
      ),
      el(
        "p",
        { class: "muted", style: "margin:8px 0 0;font-size:var(--text-xs);" },
        "渠道未勾选任何事件 = 订阅全部事件(FR-051);未绑定渠道的账号,事件发送至全部启用渠道(FR-052)。"
      )
    )
  );

  // ---- 系统 SMTP 卡(FR-053;密码只回 password_configured,留空=不修改)----
  const smtpHost = el("input", { type: "text", placeholder: "smtp.example.com" }) as HTMLInputElement;
  const smtpPort = el("input", {
    type: "number",
    min: "1",
    max: "65535",
    step: "1",
    value: String(DEFAULT_SMTP_PORT)
  }) as HTMLInputElement;
  const smtpEncryption = el("select", { "aria-label": "加密方式" }) as HTMLSelectElement;
  for (const opt of SMTP_ENCRYPTIONS) {
    smtpEncryption.append(el("option", { value: opt.value }, opt.label) as HTMLOptionElement);
  }
  const smtpUser = el("input", { type: "text", placeholder: "SMTP 账号(可选)", autocomplete: "off" }) as HTMLInputElement;
  const smtpPassword = el("input", {
    type: "password",
    placeholder: "未设置",
    autocomplete: "new-password"
  }) as HTMLInputElement;
  const smtpFromName = el("input", { type: "text", placeholder: "轻交付通知(可选)" }) as HTMLInputElement;
  const smtpFromAddress = el("input", { type: "text", placeholder: "notice@example.com" }) as HTMLInputElement;
  const smtpState = el("span", {}, "");
  const smtpError = el("div", { class: "field-error" }, "");
  const smtpSaveBtn = btn("保存系统 SMTP", { variant: "primary" });

  const smtpCard = el(
    "section",
    { class: "card" },
    el("h2", { class: "card-title" }, "系统 SMTP "),
    smtpState,
    el(
      "p",
      { class: "muted", style: "margin:0 0 12px;font-size:var(--text-xs);" },
      "邮件渠道勾选「使用系统 SMTP」时由此处统一发信;端口默认 587(STARTTLS),SSL 常用 465。"
    ),
    el(
      "div",
      { class: "notify-smtp-grid" },
      el("div", { class: "field" }, el("label", {}, "SMTP 服务器"), smtpHost),
      el("div", { class: "field" }, el("label", {}, "端口(1—65535)"), smtpPort),
      el("div", { class: "field" }, el("label", {}, "加密方式"), smtpEncryption),
      el("div", { class: "field" }, el("label", {}, "账号"), smtpUser),
      el(
        "div",
        { class: "field" },
        el("label", {}, "密码"),
        smtpPassword,
        el("div", { class: "hint" }, "已配置时留空保存表示不修改")
      ),
      el("div", { class: "field" }, el("label", {}, "发件人名称"), smtpFromName),
      el("div", { class: "field" }, el("label", {}, "发件地址"), smtpFromAddress)
    ),
    smtpError,
    el("div", { style: "display:flex;gap:12px;justify-content:flex-end;" }, smtpSaveBtn)
  );
  root.append(smtpCard);

  /** 表单常驻,加载/保存只填充取值(与设置页只读信息卡不同的写操作卡)。 */
  const fillSmtpForm = (dto: ReturnType<typeof parseSystemSmtp>): void => {
    smtpHost.value = dto.host;
    smtpPort.value = String(dto.port);
    smtpEncryption.value =
      dto.encryption === "none" || dto.encryption === "starttls" || dto.encryption === "ssl"
        ? dto.encryption
        : "starttls";
    smtpUser.value = dto.username ?? "";
    smtpFromName.value = dto.fromName ?? "";
    smtpFromAddress.value = dto.fromAddress;
    smtpPassword.value = "";
    smtpPassword.placeholder = dto.passwordConfigured ? "已配置(留空不修改)" : "未设置";
    const spec: BadgeSpec = dto.configured
      ? { tone: "normal", label: "已配置" }
      : { tone: "neutral", label: "未配置" };
    smtpState.replaceChildren(badgeEl(spec, String(dto.configured)));
  };

  const loadSmtp = async (): Promise<void> => {
    try {
      fillSmtpForm(parseSystemSmtp(await httpGet("/api/v1/settings/system-smtp")));
      smtpError.textContent = "";
    } catch (e) {
      smtpError.textContent = `系统 SMTP 加载失败:${apiErrorMessage(e)}`;
    }
  };
  void loadSmtp();

  smtpSaveBtn.addEventListener("click", async () => {
    const built = buildSystemSmtpPayload({
      host: smtpHost.value,
      port: smtpPort.value,
      encryption: smtpEncryption.value,
      username: smtpUser.value,
      password: smtpPassword.value,
      fromName: smtpFromName.value,
      fromAddress: smtpFromAddress.value
    });
    if (!built.ok) {
      smtpError.textContent = built.error;
      return;
    }
    smtpSaveBtn.disabled = true;
    try {
      fillSmtpForm(parseSystemSmtp(await httpPut("/api/v1/settings/system-smtp", built.payload)));
      smtpError.textContent = "";
    } catch (e) {
      smtpError.textContent = `保存失败:${apiErrorMessage(e)}`;
    } finally {
      smtpSaveBtn.disabled = false;
    }
  });

  // ---- 渠道卡列表 ----
  const channelCard = (c: NotificationChannelDto): HTMLElement => {
    const testBtn = btn("测试", { variant: "secondary" });
    testBtn.classList.add("btn-sm");
    const editBtn = el("button", { class: "icon-btn", type: "button", title: `编辑 ${c.name}` });
    editBtn.append(icon("edit"));
    const delBtn = el("button", { class: "icon-btn danger-icon", type: "button", title: `删除 ${c.name}` });
    delBtn.append(icon("trash"));
    const testLine = el("div", { class: "notify-test-line", style: "display:none;" }, "");

    // 启用开关:整组乐观锁更新(config 原样回传;secrets 空包=不修改)
    const sw = el("input", { type: "checkbox", role: "switch", "aria-label": `启用 ${c.name}` }) as HTMLInputElement;
    sw.checked = c.enabled;
    sw.addEventListener("change", async () => {
      const enabling = sw.checked;
      sw.disabled = true;
      try {
        await httpPut(`/api/v1/notification-channels/${c.id}`, {
          kind: c.kindRaw,
          name: c.name,
          enabled: enabling,
          config: c.config,
          secrets: {},
          event_types: c.eventTypes,
          expected_version: c.version
        });
        channelsBlockRef?.reload();
      } catch (e) {
        sw.checked = !enabling; // 失败回滚视觉状态
        confirmDialog({ title: "操作失败", message: `切换启用状态失败:${apiErrorMessage(e)}`, confirmLabel: "知道了" });
      } finally {
        sw.disabled = false;
      }
    });

    // 测试投递:按钮转圈 → 行内结果(成功/结构化失败原因/结果未知,errorHint 兜底)
    testBtn.addEventListener("click", async () => {
      testBtn.disabled = true;
      const label = testBtn.textContent;
      testBtn.textContent = "测试中…";
      testLine.style.display = "block";
      testLine.replaceChildren(el("span", { class: "muted" }, "正在发送测试通知…"));
      try {
        const r = parseTestSendResult(await httpPost(`/api/v1/notification-channels/${c.id}/test`));
        testLine.replaceChildren(el("span", { class: testResultTone(r) }, testResultText(r)));
      } catch (e) {
        testLine.replaceChildren(
          el("span", { class: "error-text" }, `测试请求失败:${apiErrorMessage(e)}`)
        );
      } finally {
        testBtn.disabled = false;
        testBtn.textContent = label;
      }
    });

    editBtn.addEventListener("click", () => {
      openChannelEditor(editBtn, c, { onSaved: () => channelsBlockRef?.reload() });
    });
    delBtn.addEventListener("click", () => {
      openConfirmFlow({
        title: `删除渠道「${c.name}」?`,
        impact: el(
          "div",
          {},
          el("p", { class: "error-text", style: "margin:0 0 6px;font-weight:600;" }, "此操作无法撤销"),
          el(
            "p",
            { class: "muted", style: "margin:0;" },
            "删除后该渠道配置与已保存的秘密将被清除,各账号对它的绑定关系一并解除;历史投递留痕保留。"
          )
        ),
        requireReason: false,
        requireRiskAck: false,
        confirmLabel: "确认删除",
        danger: true,
        submit: async () => {
          try {
            await httpRequest<void>(`/api/v1/notification-channels/${c.id}`, { method: "DELETE" });
          } catch (e) {
            throw new Error(apiErrorMessage(e));
          }
        },
        onSuccess: () => {
          channelsBlockRef?.reload();
          bindingsBlockRef?.reload();
        }
      });
    });

    const secretsLine =
      c.secretsConfigured.length > 0
        ? el(
            "div",
            { class: "notify-card-sub" },
            "秘密:已配置 ",
            el("span", { class: "mono" }, c.secretsConfigured.map(secretLabel).join(" · "))
          )
        : null;

    return el(
      "article",
      { class: "notify-card" },
      el(
        "div",
        { class: "notify-card-head" },
        el(
          "div",
          { class: "notify-card-title" },
          el("span", { class: "notify-card-icon", "aria-hidden": "true" }, KIND_ICONS[c.kind]),
          el("strong", { class: "notify-card-name", title: c.name }, c.name),
          badgeEl({ tone: KIND_TONES[c.kind], label: KIND_LABELS[c.kind] }, c.kindRaw)
        ),
        el("label", { class: "check-row", title: "启用后参与事件发送" }, sw, "启用")
      ),
      el("div", { class: "notify-card-meta mono" }, configSummary(c.kind, c.config)),
      el("div", { class: "notify-card-sub" }, "订阅:", el("strong", {}, subscriptionSummary(c.eventTypes))),
      secretsLine,
      testLine,
      el("div", { class: "notify-card-actions" }, testBtn, editBtn, delBtn)
    );
  };

  root.append(el("h2", { class: "notify-section-title" }, "通知渠道"));
  const channelsHost = el("div", { class: "notify-list-host" });
  root.append(channelsHost);
  const channelsBlock = asyncBlock(
    channelsHost,
    async () => {
      const page = parsePage(await httpGet("/api/v1/notification-channels"), parseChannel);
      channels = page.items;
      if (channels.length === 0) return null;
      return el("div", { class: "notify-grid" }, ...channels.map(channelCard));
    },
    {
      skeletonLines: 4,
      empty: () =>
        el(
          "div",
          { class: "empty-state" },
          el("div", { class: "empty-ico" }, "◆"),
          el("div", {}, "还没有通知渠道"),
          el(
            "div",
            { class: "muted" },
            "点击「新建渠道」配置 Webhook、邮件、钉钉、飞书、企业微信、Bark 或 Telegram"
          )
        )
    }
  );
  channelsBlockRef = channelsBlock;

  // ---- 账号绑定区(FR-052 覆盖式;未绑定=全部启用渠道)----
  /** 单账号绑定行:渠道复选组 + 覆盖式保存(勾选集合整体 PUT)。 */
  const bindingRow = (row: {
    account: AccountLite;
    bound: string[];
    loadError: string | null;
  }): HTMLElement => {
    const boundSet = new Set(row.bound);
    const status = el(
      "span",
      { class: "muted", style: "font-size:var(--text-xs);" },
      row.loadError !== null ? `绑定加载失败:${row.loadError}` : bindingSummary(row.bound)
    );
    const saveBtn = btn("保存", { variant: "secondary" });
    saveBtn.classList.add("btn-sm");
    saveBtn.disabled = true;
    const checks = new Map<string, HTMLInputElement>();
    const checksBox = el("div", { class: "notify-bind-channels" });
    for (const c of channels) {
      const check = el("input", { type: "checkbox" }) as HTMLInputElement;
      check.checked = boundSet.has(c.id);
      checks.set(c.id, check);
      checksBox.append(el("label", { class: "check-row" }, check, `${c.name}(${KIND_LABELS[c.kind]})`));
    }
    checksBox.addEventListener("change", () => {
      saveBtn.disabled = false;
      status.textContent = "有未保存的修改";
    });
    saveBtn.addEventListener("click", async () => {
      const ids = uniqueChannelIds(
        [...checks.entries()].filter(([, box]) => box.checked).map(([id]) => id)
      );
      saveBtn.disabled = true;
      try {
        await httpPut(`/api/v1/accounts/${row.account.id}/notification-bindings`, {
          channel_ids: ids
        });
        row.bound = ids;
        status.textContent = `已保存 · ${bindingSummary(ids)}`;
      } catch (e) {
        status.textContent = `保存失败:${apiErrorMessage(e)}`;
        saveBtn.disabled = false;
      }
    });
    return el(
      "div",
      { class: "notify-bind-row" },
      el(
        "div",
        { class: "notify-bind-account" },
        el("strong", {}, row.account.displayName),
        el("span", { class: "muted mono", style: "font-size:var(--text-xs);" }, row.account.id)
      ),
      checksBox,
      el("div", { class: "notify-bind-foot" }, status, saveBtn)
    );
  };

  root.append(el("h2", { class: "notify-section-title" }, "账号绑定"));
  const bindingsHost = el("div", { class: "notify-bind-host" });
  root.append(bindingsHost);
  const bindingsBlock = asyncBlock(
    bindingsHost,
    async () => {
      const raw = await httpGet("/api/v1/accounts");
      const items = isRecord(raw) && Array.isArray(raw["items"]) ? raw["items"] : [];
      const accounts: AccountLite[] = items.map((a): AccountLite => {
        const r = isRecord(a) ? a : {};
        return {
          id: String(r["id"] ?? ""),
          displayName: String(r["display_name"] ?? r["id"] ?? ""),
          connectionState: String(r["connection_state"] ?? "paused")
        };
      });
      if (accounts.length === 0) return null;
      // 复选组依赖渠道清单:与渠道块并行加载时缓存可能尚未就绪,兜底自取一次
      if (channels.length === 0) {
        try {
          channels = parsePage(await httpGet("/api/v1/notification-channels"), parseChannel).items;
        } catch {
          channels = []; // 渠道清单不可用:按"暂无渠道"如实展示,不臆造
        }
      }
      const rows = await Promise.all(
        accounts.map(async (a) => {
          let bound: string[] = [];
          let loadError: string | null = null;
          try {
            bound = parseBindings(await httpGet(`/api/v1/accounts/${a.id}/notification-bindings`));
          } catch (e) {
            loadError = e instanceof Error ? e.message : String(e);
          }
          return { account: a, bound, loadError };
        })
      );
      return el(
        "section",
        { class: "card" },
        el("h2", { class: "card-title" }, "按账号绑定渠道"),
        el(
          "p",
          { class: "muted", style: "margin:0 0 12px;font-size:var(--text-xs);" },
          "勾选该账号的事件发送到哪些渠道(覆盖式保存);不勾选任何渠道 = 发送至全部启用渠道。"
        ),
        channels.length === 0
          ? el("p", { class: "muted" }, "暂无渠道可绑定:先在上方创建通知渠道。")
          : el("div", { class: "notify-bind-list" }, ...rows.map(bindingRow))
      );
    },
    {
      skeletonLines: 3,
      empty: () =>
        el(
          "div",
          { class: "empty-state" },
          el("div", { class: "empty-ico" }, "◉"),
          el("div", {}, "暂无账号"),
          el("div", { class: "muted" }, "添加闲鱼账号后可按账号绑定通知渠道")
        )
    }
  );
  bindingsBlockRef = bindingsBlock;

  // ---- 工具栏动作 ----
  addBtn.addEventListener("click", () => {
    openChannelEditor(addBtn, null, {
      onSaved: () => {
        channelsBlockRef?.reload();
        bindingsBlockRef?.reload();
      }
    });
  });
  refreshBtn.addEventListener("click", () => {
    void loadSmtp();
    channelsBlockRef?.reload();
    bindingsBlockRef?.reload();
  });

  return () => {
    channelsBlock.dispose();
    bindingsBlock.dispose();
  };
};

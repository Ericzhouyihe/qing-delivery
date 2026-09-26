/** 渠道编辑弹窗(007 T064,FR-050/FR-051):名称 + 类型七选一(选中切换字段区,
 *  参照卡密编辑器的类型切换)+ 各类型字段与"如何获取配置"折叠指引 +
 *  事件订阅六选(全不选=全部,UI 明示)。
 *  契约不变量:秘密字段(secret/device_key/bot_token/SMTP 凭据)编辑已有渠道时
 *  显示"已配置"占位、留空=不修改(后端空包语义);创建时必填键不可为空。 */
import { el } from "../../shared/dom";
import { httpPost, httpPut } from "../../shared/http";
import { openModal } from "../../ui/modal";
import { btn } from "../../ui/dom";
import {
  CHANNEL_KINDS,
  EVENT_LABELS,
  EVENT_NOTES,
  EVENT_TYPES,
  KIND_GUIDES,
  KIND_LABELS,
  buildChannelPayload,
  validateChannelName,
  type ChannelFormInput,
  type ChannelKind,
  type EventType,
  type NotificationChannelDto
} from "./model";

export interface ChannelEditorCallbacks {
  onSaved: () => void;
}

function cfgStr(config: Record<string, unknown>, key: string): string {
  const v = config[key];
  return typeof v === "string" ? v.trim() : "";
}

/** 各类型"Webhook 地址"字段的标签口径(webhook=回调,IM 机器人=机器人地址)。 */
function urlFieldLabel(kind: ChannelKind): string {
  return kind === "webhook" ? "回调 URL" : "机器人 Webhook 地址";
}

export function openChannelEditor(
  opener: HTMLElement | null,
  existing: NotificationChannelDto | null,
  cb: ChannelEditorCallbacks
): void {
  let dirty = false;
  const creating = existing === null;
  const handle = openModal(opener, {
    title: creating ? "新建通知渠道" : `编辑渠道 · ${existing!.name.slice(0, 16)}`,
    isDirty: () => dirty
  });
  // 字段区较宽(类型切换 + 订阅组),沿用宽抽屉的加宽手法
  handle.body.closest(".modal")?.classList.add("modal-wide");

  let kind: ChannelKind = existing?.kind ?? "webhook";
  const configured = (key: string): boolean =>
    existing?.secretsConfigured.includes(key) === true;
  const secretPlaceholder = (configuredLabel: string, blank: string): string =>
    creating || !configured(configuredLabel) ? blank : "已配置(不回显);留空保持不变";

  // ---- 通用字段(三件套:field + hint + field-error)----
  const nameInput = el("input", { type: "text", placeholder: "渠道名称(必填,≤100 字)" }) as HTMLInputElement;
  nameInput.value = existing?.name ?? "";
  const nameError = el("div", { class: "field-error" }, "");
  const enabledCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  enabledCheck.checked = existing?.enabled ?? true;
  const configError = el("div", { class: "field-error" }, "");
  const saveError = el("div", { class: "field-error" }, "");
  const saveBtn = btn(creating ? "创建" : "保存", { variant: "primary" });

  // ---- 类型选择(chip 组;编辑态锁定——切换类型会使 config/secrets 失配)----
  const kindBox = el("div", { class: "range-chips", style: "margin-bottom:0;" });
  const kindBtns = new Map<ChannelKind, HTMLButtonElement>();
  const syncKindActive = (): void => {
    for (const [k, b] of kindBtns) {
      const active = k === kind;
      b.classList.toggle("active", active);
      b.setAttribute("aria-pressed", active ? "true" : "false");
    }
  };
  for (const k of CHANNEL_KINDS) {
    const b = el("button", { class: "chip", type: "button", "aria-pressed": "false" }, KIND_LABELS[k]) as HTMLButtonElement;
    if (!creating) {
      b.disabled = true;
      b.title = "类型创建后不可更改";
    } else {
      b.addEventListener("click", () => {
        kind = k;
        dirty = true;
        syncKindActive();
        syncKindFields();
        refreshValidation();
      });
    }
    kindBtns.set(k, b);
    kindBox.append(b);
  }
  syncKindActive();

  // ---- 各类型字段(config 明文可回显;秘密永不回显)----
  const urlLabelEl = el("label", {}, urlFieldLabel(kind));
  const urlInput = el("input", { type: "text", placeholder: "https://…" }) as HTMLInputElement;
  urlInput.value = creating ? "" : cfgStr(existing!.config, "url");

  const secretInput = el("input", {
    type: "text",
    placeholder: secretPlaceholder("secret", "加签/签名密钥(可选;未开启加签则留空)"),
    autocomplete: "off"
  }) as HTMLInputElement;

  const serverUrlInput = el("input", {
    type: "text",
    placeholder: "https://api.day.app(自建服务端可修改)"
  }) as HTMLInputElement;
  serverUrlInput.value = creating ? "" : cfgStr(existing!.config, "server_url");
  const deviceKeyInput = el("input", {
    type: "text",
    placeholder: secretPlaceholder("device_key", "Bark App 页面显示的设备 Key"),
    autocomplete: "off"
  }) as HTMLInputElement;

  const botTokenInput = el("input", {
    type: "text",
    placeholder: secretPlaceholder("bot_token", "123456789:AA…(BotFather 发放的 Token)"),
    autocomplete: "off"
  }) as HTMLInputElement;
  const chatIdInput = el("input", { type: "text", placeholder: "数字 / @频道名 / -100 开头群组" }) as HTMLInputElement;
  chatIdInput.value = creating ? "" : cfgStr(existing!.config, "chat_id");

  const toAddressInput = el("input", { type: "text", placeholder: "通知发送到该邮箱" }) as HTMLInputElement;
  toAddressInput.value = creating ? "" : cfgStr(existing!.config, "to_address");
  const useSystemSmtpCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  // 后端口径:use_system_smtp 缺失按 false(独立 SMTP);仅显式 true 才复用系统发信
  useSystemSmtpCheck.checked = creating ? true : existing!.config["use_system_smtp"] === true;
  const smtpHostInput = el("input", { type: "text", placeholder: "smtp.example.com" }) as HTMLInputElement;
  smtpHostInput.value = creating ? "" : cfgStr(existing!.config, "smtp_host");
  const smtpPortInput = el("input", { type: "number", min: "1", max: "65535", step: "1", value: "587" }) as HTMLInputElement;
  if (!creating && existing!.config["smtp_port"] !== undefined) {
    smtpPortInput.value = String(existing!.config["smtp_port"]);
  }
  const smtpUserInput = el("input", {
    type: "text",
    placeholder: secretPlaceholder("smtp_user", "SMTP 账号(匿名中继可留空)"),
    autocomplete: "off"
  }) as HTMLInputElement;
  const smtpPasswordInput = el("input", {
    type: "password",
    placeholder: secretPlaceholder("smtp_password", "SMTP 密码(匿名中继可留空)"),
    autocomplete: "new-password"
  }) as HTMLInputElement;
  const fromAddressInput = el("input", { type: "text", placeholder: "shop@example.com" }) as HTMLInputElement;
  fromAddressInput.value = creating ? "" : cfgStr(existing!.config, "from_address");

  // ---- 事件订阅(六选;全不选=全部)----
  const eventChecks = new Map<EventType, HTMLInputElement>();
  const initialEvents = new Set<EventType>(existing?.eventTypes ?? []);
  const eventBox = el("div", { class: "notify-events" });
  for (const t of EVENT_TYPES) {
    const check = el("input", { type: "checkbox" }) as HTMLInputElement;
    check.checked = initialEvents.has(t);
    check.addEventListener("change", () => {
      dirty = true;
    });
    eventChecks.set(t, check);
    eventBox.append(
      el("label", { class: "check-row notify-event-check" },
        check,
        el("span", {}, EVENT_LABELS[t]),
        el("span", { class: "muted", style: "font-size:var(--text-xs);" }, EVENT_NOTES[t]))
    );
  }

  // ---- 按类型显隐的字段容器 + "如何获取配置"折叠指引 ----
  const guideEl = el("details", { class: "notify-guide" });
  const syncGuide = (): void => {
    guideEl.replaceChildren(
      el("summary", {}, "如何获取配置?"),
      el("div", { class: "notify-guide-body" }, KIND_GUIDES[kind])
    );
  };

  const field = (label: HTMLElement | string, control: HTMLElement, ...rest: HTMLElement[]): HTMLElement =>
    el("div", { class: "field" }, typeof label === "string" ? el("label", {}, label) : label, control, ...rest);

  // 注意:urlInput 等控件只归属一个容器(append 会移动节点);
  // webhook/dingtalk/feishu/wecom 共用同一 URL 字段组,加签另设独立组按需显隐。
  const urlFields = el("div", {}, field(urlLabelEl, urlInput));
  const secretLabelEl = el("label", {}, "加签 secret");
  const secretFields = el("div", {}, field(secretLabelEl, secretInput));
  const barkFields = el(
    "div",
    {},
    field("服务器地址", serverUrlInput),
    field("device_key", deviceKeyInput, el("div", { class: "hint" }, "在 iPhone 的 Bark App 中查看"))
  );
  const telegramFields = el(
    "div",
    {},
    field("bot_token", botTokenInput),
    field("chat_id", chatIdInput, el("div", { class: "hint" }, "接收通知的会话:个人数字 id、@频道名或 -100 开头群组"))
  );
  const emailStandalone = el(
    "div",
    { class: "notify-email-smtp" },
    field("SMTP 服务器", smtpHostInput),
    field("SMTP 端口(1—65535)", smtpPortInput),
    field("SMTP 账号", smtpUserInput),
    field("SMTP 密码", smtpPasswordInput),
    field("发件地址", fromAddressInput)
  );
  const emailFields = el(
    "div",
    {},
    field("收件邮箱", toAddressInput),
    el("label", { class: "check-row" }, useSystemSmtpCheck, "使用系统 SMTP(由「通知设置」页系统 SMTP 卡统一发信)"),
    el(
      "div",
      { class: "hint", style: "margin-top:4px;" },
      useSystemSmtpCheck.checked ? "已启用系统 SMTP:只需填写收件邮箱" : "独立 SMTP:填写该渠道专用的发信服务器与发件地址"
    ),
    emailStandalone
  );
  const syncEmailHint = (): void => {
    const hint = emailFields.querySelector(".hint");
    if (hint !== null) {
      hint.textContent = useSystemSmtpCheck.checked
        ? "已启用系统 SMTP:只需填写收件邮箱"
        : "独立 SMTP:填写该渠道专用的发信服务器与发件地址";
    }
    emailStandalone.hidden = useSystemSmtpCheck.checked;
  };
  useSystemSmtpCheck.addEventListener("change", () => {
    dirty = true;
    syncEmailHint();
    refreshValidation();
  });

  /** 各类型 → 需要显示的字段组(webhook/dingtalk/feishu/wecom 共用 URL 组)。 */
  const kindGroups: Record<ChannelKind, HTMLElement[]> = {
    webhook: [urlFields],
    dingtalk: [urlFields, secretFields],
    feishu: [urlFields, secretFields],
    wecom: [urlFields],
    bark: [barkFields],
    telegram: [telegramFields],
    email: [emailFields]
  };
  const allKindGroups = [urlFields, secretFields, barkFields, telegramFields, emailFields];
  for (const group of allKindGroups) group.hidden = true;
  const syncKindFields = (): void => {
    const visible = new Set(kindGroups[kind]);
    for (const group of allKindGroups) group.hidden = !visible.has(group);
    urlLabelEl.textContent = urlFieldLabel(kind);
    secretLabelEl.textContent = kind === "dingtalk" ? "加签 secret" : "签名 secret";
    syncGuide();
  };
  syncKindFields();
  syncEmailHint();

  // ---- 行内实时校验(名称独立;其余逐项由载荷构建器汇总)----
  const collect = (): ChannelFormInput => ({
    kind,
    name: nameInput.value,
    enabled: enabledCheck.checked,
    url: urlInput.value,
    secret: secretInput.value,
    serverUrl: serverUrlInput.value,
    deviceKey: deviceKeyInput.value,
    botToken: botTokenInput.value,
    chatId: chatIdInput.value,
    toAddress: toAddressInput.value,
    useSystemSmtp: useSystemSmtpCheck.checked,
    smtpHost: smtpHostInput.value,
    smtpPort: smtpPortInput.value,
    smtpUser: smtpUserInput.value,
    smtpPassword: smtpPasswordInput.value,
    fromAddress: fromAddressInput.value,
    eventTypes: EVENT_TYPES.filter((t) => eventChecks.get(t)?.checked === true)
  });

  const refreshValidation = (): void => {
    const name = validateChannelName(nameInput.value);
    nameError.textContent = name.message;
    nameInput.setAttribute("aria-invalid", name.ok ? "false" : "true");
    const built = buildChannelPayload(collect(), { secretsRequired: creating });
    configError.textContent = built.ok ? "" : built.error;
    saveBtn.disabled = !(name.ok && built.ok);
  };
  for (const node of [
    nameInput,
    urlInput,
    secretInput,
    serverUrlInput,
    deviceKeyInput,
    botTokenInput,
    chatIdInput,
    toAddressInput,
    smtpHostInput,
    smtpPortInput,
    smtpUserInput,
    smtpPasswordInput,
    fromAddressInput
  ]) {
    node.addEventListener("input", () => {
      dirty = true;
      refreshValidation();
    });
  }
  enabledCheck.addEventListener("change", () => {
    dirty = true;
  });
  refreshValidation();

  // ---- 保存:POST 创建 / PUT 更新(乐观锁 expected_version;secrets 留空=不修改)----
  saveBtn.addEventListener("click", async () => {
    refreshValidation();
    if (saveBtn.disabled) return;
    saveBtn.disabled = true;
    const built = buildChannelPayload(collect(), { secretsRequired: creating });
    if (!built.ok) {
      configError.textContent = built.error;
      saveBtn.disabled = false;
      return;
    }
    try {
      if (creating) {
        await httpPost("/api/v1/notification-channels", built.payload);
      } else {
        await httpPut(`/api/v1/notification-channels/${existing!.id}`, {
          ...built.payload,
          expected_version: existing!.version
        });
      }
      dirty = false;
      handle.close(true);
      cb.onSaved();
    } catch (e) {
      saveError.textContent = `保存失败:${e instanceof Error ? e.message : String(e)}`;
      saveBtn.disabled = false;
    }
  });

  // ---- 装配 ----
  handle.body.append(
    field("名称", nameInput, nameError),
    field("类型", kindBox, el("div", { class: "hint" }, creating ? "七类渠道;创建后类型不可更改" : "类型已锁定,不可更改")),
    urlFields,
    secretFields,
    barkFields,
    telegramFields,
    emailFields,
    el("hr", { class: "notify-sep" }),
    el(
      "div",
      { class: "field" },
      el("label", {}, "事件订阅"),
      el("div", { class: "hint" }, "勾选该渠道接收的事件;全部不勾选 = 订阅全部事件"),
      eventBox
    ),
    guideEl,
    saveError,
    el("div", { style: "display:flex;gap:12px;justify-content:flex-end;margin-top:12px;" }, saveBtn)
  );
}

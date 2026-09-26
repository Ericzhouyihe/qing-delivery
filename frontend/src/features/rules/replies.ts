/** 关键词回复编辑弹窗 + 账号默认回复编辑弹窗(007 T040)。
 *  关键词:关键词必填、关联商品多选(搜索,不选=账号级)、回复类型 radio 文字/图片;
 *  默认回复:启用开关/回复文字/图片 URL 可选/只回复一次/清空回复记录(confirm-flow)。
 *  校验与负载组装在 model.ts(纯函数);弹窗只装配。 */
import { el } from "../../shared/dom";
import { httpPost, httpPut, httpRequest } from "../../shared/http";
import { openModal } from "../../ui/modal";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { btn } from "../../ui/dom";
import type { ProductItem } from "../catalog/products";
import {
  buildReplyRulePayload,
  validateDefaultReplyForm,
  validateReplyRuleForm,
  type DefaultReplyDto,
  type ReplyRuleDto,
  type ReplyRuleFormValue
} from "./model";

const field = (label: string, control: HTMLElement, ...hints: HTMLElement[]): HTMLElement =>
  el("div", { class: "field" }, el("label", {}, label), control, ...hints);

/** 关键词回复编辑弹窗:existing=null 为新建。 */
export function openReplyRuleEditor(
  opener: HTMLElement | null,
  accountId: string,
  items: readonly ProductItem[],
  existing: ReplyRuleDto | null,
  onSaved: () => void
): void {
  const creating = existing === null;
  let dirty = false;
  const handle = openModal(opener, {
    title: creating ? "新建关键词回复" : `编辑关键词回复 · ${existing!.keyword.slice(0, 12)}`,
    isDirty: () => dirty
  });

  const value: ReplyRuleFormValue = {
    keyword: existing?.keyword ?? "",
    replyKind: existing?.replyKind === "image" ? "image" : "text",
    replyText: existing?.replyText ?? "",
    replyImageUrl: existing?.replyImageUrl ?? "",
    enabled: existing?.enabled ?? true,
    itemIds: existing?.itemIds.slice() ?? []
  };

  const keywordInput = el("input", { type: "text", placeholder: "买家消息包含该关键词时触发(必填)" }) as HTMLInputElement;
  keywordInput.value = value.keyword;
  const kindText = el("input", { type: "radio", name: "reply-kind" }) as HTMLInputElement;
  kindText.value = "text";
  const kindImage = el("input", { type: "radio", name: "reply-kind" }) as HTMLInputElement;
  kindImage.value = "image";
  const kindRadios = [kindText, kindImage];
  if (value.replyKind === "image") kindImage.checked = true;
  else kindText.checked = true;
  const textArea = el("textarea", { rows: "3", placeholder: "回复文案(文字回复必填)" }) as HTMLTextAreaElement;
  textArea.value = value.replyText;
  textArea.style.minHeight = "64px";
  const imageInput = el("input", { type: "text", placeholder: "图片链接(图片回复必填)" }) as HTMLInputElement;
  imageInput.value = value.replyImageUrl;
  const enabledCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  enabledCheck.checked = value.enabled;

  // 关联商品多选:搜索过滤 + 复选列表(不勾选任何项 = 账号级)
  const itemSearch = el("input", { type: "search", placeholder: "搜索商品标题 / 平台 ID" }) as HTMLInputElement;
  const checkHost = el("div", { class: "reply-item-picker" });
  const checked = new Set<string>(value.itemIds);
  const renderChecks = (): void => {
    const q = itemSearch.value.trim().toLowerCase();
    const filtered = items.filter(
      (it) => q === "" || it.title.toLowerCase().includes(q) || it.platformItemId.toLowerCase().includes(q)
    );
    checkHost.replaceChildren();
    if (items.length === 0) {
      checkHost.append(el("div", { class: "muted", style: "font-size:var(--text-xs);" }, "该账号暂无同步商品;不关联商品 = 账号级"));
      return;
    }
    for (const it of filtered) {
      const box = el("input", { type: "checkbox" }) as HTMLInputElement;
      box.checked = checked.has(it.id);
      box.addEventListener("change", () => {
        if (box.checked) checked.add(it.id);
        else checked.delete(it.id);
        syncScopeHint();
        markDirty();
      });
      checkHost.append(el("label", { class: "check-row" }, box, it.title.slice(0, 30)));
    }
    if (filtered.length === 0) {
      checkHost.append(el("div", { class: "muted", style: "font-size:var(--text-xs);" }, "没有匹配的商品"));
    }
  };
  itemSearch.addEventListener("input", renderChecks);
  const scopeHint = el("div", { class: "hint" }, "");
  const syncScopeHint = (): void => {
    scopeHint.textContent = checked.size === 0 ? "未勾选任何商品 = 账号级(对该账号全部商品生效)" : `已关联 ${checked.size} 个商品;商品级关键词优先于账号级`;
  };

  const errorSummary = el("div", { class: "field-error" }, "");
  const saveError = el("div", { class: "field-error" }, "");
  const saveBtn = btn(creating ? "创建" : "保存", { variant: "primary" });

  const markDirty = (): void => {
    dirty = true;
  };
  const gate = (): void => {
    const errors = validateReplyRuleForm({
      ...value,
      keyword: keywordInput.value,
      replyText: textArea.value,
      replyImageUrl: imageInput.value,
      itemIds: [...checked]
    });
    saveBtn.disabled = errors.length > 0;
    errorSummary.textContent = errors.join(";");
  };
  const syncKindFields = (): void => {
    textArea.disabled = false;
    imageInput.disabled = false;
    gate();
  };
  for (const radio of kindRadios) {
    radio.addEventListener("change", () => {
      value.replyKind = kindImage.checked ? "image" : "text";
      markDirty();
      syncKindFields();
    });
  }
  keywordInput.addEventListener("input", () => {
    markDirty();
    gate();
  });
  textArea.addEventListener("input", () => {
    markDirty();
    gate();
  });
  imageInput.addEventListener("input", () => {
    markDirty();
    gate();
  });
  enabledCheck.addEventListener("change", markDirty);

  saveBtn.addEventListener("click", async () => {
    const payload = buildReplyRulePayload({
      keyword: keywordInput.value,
      replyKind: kindImage.checked ? "image" : "text",
      replyText: textArea.value,
      replyImageUrl: imageInput.value,
      enabled: enabledCheck.checked,
      itemIds: [...checked]
    });
    saveBtn.disabled = true;
    saveError.textContent = "";
    try {
      if (creating) {
        await httpPost(`/api/v1/accounts/${accountId}/reply-rules`, payload);
      } else {
        await httpPut(`/api/v1/accounts/${accountId}/reply-rules/${existing!.id}`, payload);
      }
      dirty = false;
      handle.close(true);
      onSaved();
    } catch (e) {
      saveError.textContent = `保存失败:${e instanceof Error ? e.message : String(e)}`;
      saveBtn.disabled = false;
    }
  });

  renderChecks();
  syncScopeHint();
  gate();
  handle.body.append(
    field("关键词", keywordInput, el("div", { class: "hint" }, "包含匹配、不区分大小写;命中即回复,不触发发货、不改订单状态")),
    field("回复类型", el("div", { style: "display:flex;gap:16px;" },
      el("label", { class: "check-row" }, kindText, "回复文字"),
      el("label", { class: "check-row" }, kindImage, "回复图片"))),
    field("回复文案(文字)", textArea),
    field("图片链接(图片)", imageInput),
    field("关联商品(多选,可不选)", itemSearch, checkHost, scopeHint),
    el("label", { class: "check-row" }, enabledCheck, "启用该关键词规则"),
    errorSummary,
    saveError
  );
  // 弹窗无独立 foot 区,操作行置于体末(modal 既有模式)
  handle.body.append(el("div", { style: "display:flex;gap:12px;justify-content:flex-end;" }, saveBtn));
}

/** 账号默认回复编辑弹窗:启用/内容/只回复一次 + 清空回复记录(confirm-flow)。 */
export function openDefaultReplyEditor(
  opener: HTMLElement | null,
  accountLabel: string,
  accountId: string,
  existing: DefaultReplyDto,
  onSaved: () => void
): void {
  let dirty = false;
  const handle = openModal(opener, {
    title: `账号默认回复 · ${accountLabel.slice(0, 16)}`,
    isDirty: () => dirty
  });

  const enabledCheck = el("input", { type: "checkbox", role: "switch" }) as HTMLInputElement;
  enabledCheck.checked = existing.enabled;
  const textArea = el("textarea", { rows: "3", placeholder: "兜底回复文案(未命中关键词与 AI 时)" }) as HTMLTextAreaElement;
  textArea.value = existing.replyText ?? "";
  textArea.style.minHeight = "64px";
  const imageInput = el("input", { type: "text", placeholder: "图片链接(可选)" }) as HTMLInputElement;
  imageInput.value = existing.replyImageUrl ?? "";
  const onceCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  onceCheck.checked = existing.replyOnce;

  const errorSummary = el("div", { class: "field-error" }, "");
  const saveError = el("div", { class: "field-error" }, "");
  const saveBtn = btn("保存", { variant: "primary" });
  const clearBtn = btn("清空回复记录", { variant: "secondary" });

  const markDirty = (): void => {
    dirty = true;
  };
  const gate = (): void => {
    const errors = validateDefaultReplyForm({
      enabled: enabledCheck.checked,
      replyText: textArea.value,
      replyImageUrl: imageInput.value
    });
    saveBtn.disabled = errors.length > 0;
    errorSummary.textContent = errors.join(";");
  };
  enabledCheck.addEventListener("change", () => {
    markDirty();
    gate();
  });
  textArea.addEventListener("input", () => {
    markDirty();
    gate();
  });
  imageInput.addEventListener("input", () => {
    markDirty();
    gate();
  });
  onceCheck.addEventListener("change", markDirty);

  saveBtn.addEventListener("click", async () => {
    saveBtn.disabled = true;
    saveError.textContent = "";
    try {
      await httpPut(`/api/v1/accounts/${accountId}/default-reply`, {
        enabled: enabledCheck.checked,
        reply_text: textArea.value,
        reply_image_url: imageInput.value.trim().length === 0 ? null : imageInput.value.trim(),
        reply_once: onceCheck.checked
      });
      dirty = false;
      handle.close(true);
      onSaved();
    } catch (e) {
      saveError.textContent = `保存失败:${e instanceof Error ? e.message : String(e)}`;
      saveBtn.disabled = false;
    }
  });

  clearBtn.addEventListener("click", () => {
    openConfirmFlow({
      title: `清空「${accountLabel.slice(0, 12)}」的回复记录?`,
      impact: el(
        "div",
        {},
        el("p", { class: "error-text", style: "margin:0 0 6px;font-weight:600;" }, "此操作无法撤销"),
        el(
          "p",
          { class: "muted", style: "margin:0;" },
          `将删除该账号「只回复一次」的全部已回复标记(现有最近记录 ${existing.recentRecords.length} 条);清空后对已回复过的买家恢复默认回复。`
        )
      ),
      requireReason: false,
      requireRiskAck: false,
      confirmLabel: "确认清空",
      danger: true,
      submit: async () => {
        try {
          await httpRequest<void>(`/api/v1/accounts/${accountId}/default-reply/clear-records`, { method: "POST" });
        } catch (e) {
          throw new Error(e instanceof Error ? e.message : String(e));
        }
      },
      onSuccess: onSaved
    });
  });

  gate();
  handle.body.append(
    field("启用开关", el("label", { class: "check-row" }, enabledCheck, "启用该账号的默认回复"),
      el("div", { class: "hint" }, "买家普通消息未命中关键词(且未开启 AI)时的兜底回复;绝不触发发货或改订单状态")),
    field("回复文案", textArea),
    field("图片链接(可选)", imageInput),
    el("label", { class: "check-row" }, onceCheck, "只回复一次(同一买家仅首条消息回复)"),
    errorSummary,
    saveError,
    el("div", { style: "display:flex;gap:12px;justify-content:flex-end;" }, clearBtn, saveBtn)
  );
}

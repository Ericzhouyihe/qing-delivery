/** 规则编辑抽屉(US4/T035):完整表单 + 行内实时校验(必填/字数/占位符经服务端
 *  预演校验)+ 脏关闭确认 + 引擎匹配预览(FR-013/FR-014)。
 *  契约限制:规则无读取既有正文的端点,保存为整体替换并明示(US4 冲突警告)。 */
import { el } from "../../shared/dom";
import { httpGet, httpPost, httpPut } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { openDrawer } from "../../ui/drawer";
import { btn } from "../../ui/dom";
import { renderMatchPreview } from "./match-preview";
import type { ProductItem } from "./products";

/** 字数上限取自 capabilities content_limits(001 既有约束,不自定口径)。 */
export const CONTENT_LIMITS = { unicode_scalars: 1000, utf8_bytes: 4000 } as const;

export interface ContentValidation {
  ok: boolean;
  unicodeScalars: number;
  message: string;
}

/** 客户端最小校验(必填 + 字数);占位符合法性由服务端预演判定(不本地复现)。 */
export function validateContent(content: string): ContentValidation {
  const scalars = [...content].length;
  if (content.trim().length === 0) {
    return { ok: false, unicodeScalars: scalars, message: "内容不能为空" };
  }
  if (scalars > CONTENT_LIMITS.unicode_scalars) {
    return {
      ok: false,
      unicodeScalars: scalars,
      message: `超出字数上限(${scalars}/${CONTENT_LIMITS.unicode_scalars} 字符)`
    };
  }
  return { ok: true, unicodeScalars: scalars, message: "" };
}

export interface ExistingRule {
  id: string;
  version: number;
  enabled: boolean;
}

export interface RuleEditorCallbacks {
  onSaved: () => void;
}

export function openRuleEditor(
  opener: HTMLElement | null,
  accountId: string,
  item: ProductItem,
  existing: ExistingRule | null,
  items: ProductItem[],
  cb: RuleEditorCallbacks
): void {
  let dirty = false;
  const handle = openDrawer(opener, {
    title: existing === null ? `配置发货规则 · ${item.title.slice(0, 16)}` : `编辑发货规则 · ${item.title.slice(0, 16)}`,
    isDirty: () => dirty
  });

  const skuInput = el("input", { type: "text", placeholder: "规格键(默认规格留空)", value: "" }) as HTMLInputElement;
  const contentArea = el("textarea", { placeholder: "买家将收到的固定内容,如:链接 + 提取码" }) as HTMLTextAreaElement;
  const charCount = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, `0 / ${CONTENT_LIMITS.unicode_scalars}`);
  const contentError = el("div", { class: "field-error" }, "");
  const enabledCheck = el("input", { type: "checkbox" });
  enabledCheck.checked = existing?.enabled ?? true;
  const previewBox = el("pre", { class: "preview", style: "display:none;white-space:pre-wrap;border:1px dashed var(--border-strong);padding:8px;border-radius:6px;" });
  const previewBtn = btn("内容预演", { variant: "secondary" });
  const saveBtn = btn(existing === null ? "创建规则" : "保存(整体替换)", { variant: "primary" });
  const replaceWarn =
    existing === null
      ? null
      : el(
          "div",
          { class: "banner banner-warning" },
          el("span", {}, "⚠ 该组合已有规则;当前接口不回读既有正文,保存将整体替换内容(v" + `${existing.version}` + ")")
        );

  const refreshValidation = (): void => {
    const v = validateContent(contentArea.value);
    charCount.textContent = `${v.unicodeScalars} / ${CONTENT_LIMITS.unicode_scalars}`;
    contentError.textContent = v.message;
    contentArea.setAttribute("aria-invalid", v.ok ? "false" : "true");
    saveBtn.disabled = !v.ok;
  };
  contentArea.addEventListener("input", () => {
    dirty = true;
    refreshValidation();
  });
  skuInput.addEventListener("input", () => {
    dirty = true;
  });
  enabledCheck.addEventListener("change", () => {
    dirty = true;
  });
  refreshValidation();

  previewBtn.addEventListener("click", async () => {
    previewBtn.disabled = true;
    try {
      const res = (await httpPost(`/api/v1/accounts/${accountId}/rules/preview`, {
        item_id: item.id,
        sku_key: skuInput.value,
        content: contentArea.value
      })) as Record<string, unknown>;
      previewBox.style.display = "block";
      const violations = Array.isArray(res["violations"]) ? res["violations"] : [];
      if (res["valid"] === true) {
        previewBox.replaceChildren(
          el("div", { class: "tone-normal", style: "margin-bottom:4px;" }, "✓ 内容合法;发送内容预览:"),
          el("span", {}, String(res["rendered_text"] ?? ""))
        );
      } else {
        previewBox.replaceChildren(
          el("div", { class: "error-text", style: "margin-bottom:4px;" }, "✗ 内容不合法:"),
          ...violations.map((v) => el("div", { class: "error-text" }, String((v as Record<string, unknown>)["message"] ?? v)))
        );
      }
    } catch (e) {
      previewBox.style.display = "block";
      previewBox.replaceChildren(el("span", { class: "error-text" }, `预演失败:${e instanceof Error ? e.message : String(e)}`));
    } finally {
      previewBtn.disabled = false;
    }
  });

  saveBtn.addEventListener("click", async () => {
    const v = validateContent(contentArea.value);
    if (!v.ok) return;
    saveBtn.disabled = true;
    const body = {
      item_id: item.id,
      sku_key: skuInput.value,
      content_kind: "fixed_text",
      content: contentArea.value,
      enabled: enabledCheck.checked,
      ...(existing === null ? {} : { expected_version: existing.version })
    };
    try {
      if (existing === null) {
        await httpPost(`/api/v1/accounts/${accountId}/rules`, body);
      } else {
        await httpPut(`/api/v1/accounts/${accountId}/rules/${existing.id}`, body);
      }
      dirty = false;
      handle.close(true);
      cb.onSaved();
    } catch (e) {
      contentError.textContent = `保存失败:${e instanceof Error ? e.message : String(e)}`;
      saveBtn.disabled = false;
    }
  });

  const field = (label: string, control: HTMLElement, ...hints: HTMLElement[]): HTMLElement =>
    el("div", { class: "field" }, el("label", {}, label), control, ...hints);

  handle.body.append(
    ...(replaceWarn === null ? [] : [replaceWarn]),
    field("规格键", skuInput, el("div", { class: "hint" }, "多规格商品填完整规格组合;默认规格留空")),
    field(
      "发货内容(固定文本)",
      contentArea,
      el("div", { style: "display:flex;justify-content:space-between;" }, charCount, previewBtn),
      el("div", { class: "hint" }, "当前为纯固定文本:按原样发送给买家,不支持占位符替换")
    ),
    contentError,
    previewBox,
    el("label", { class: "check-row" }, enabledCheck, "启用该规则"),
    el("h3", { style: "margin-top:16px;" }, "匹配预览"),
    el("p", { class: "muted", style: "font-size:var(--text-xs);" }, "由服务端交付引擎判定;界面不复现匹配逻辑")
  );
  renderMatchPreview(handle.body, accountId, items, { item, skuKey: skuInput.value });
  handle.foot.append(saveBtn);
}

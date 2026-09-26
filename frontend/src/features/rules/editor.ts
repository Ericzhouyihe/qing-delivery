/** 交易自动化规则编辑抽屉(007 T040):复用 catalog/rule-editor 的 renderRuleForm
 *  表单主体(v2:触发类型/优先级/内容来源三选一/变体/求评/账号级确认/启用开关),
 *  本文件只负责抽屉宿主、保存门禁(POST/PUT + 乐观锁)与服务端错误落位。
 *  契约限制保持:规则不回读既有正文,固定文字编辑=整体替换并明示;
 *  范围(商品/规格)编辑态锁定(后端拒绝范围变更)。 */
import { el } from "../../shared/dom";
import { httpPost, httpPut } from "../../shared/http";
import { openDrawer } from "../../ui/drawer";
import { btn } from "../../ui/dom";
import {
  PRIORITY_DEFAULT,
  defaultRuleFormValue,
  renderRuleForm,
  type RuleFormRefOption
} from "../catalog/rule-editor";
import type { ProductItem } from "../catalog/products";
import { ruleToFormValue, scopeLabel, type RuleDto, type RuleRefIndex } from "./model";

export interface RuleEditorDeps {
  items: readonly ProductItem[];
  pools: readonly RuleFormRefOption[];
  templates: readonly RuleFormRefOption[];
  refs: RuleRefIndex;
}

export function openRuleEditor(
  opener: HTMLElement | null,
  accountId: string,
  deps: RuleEditorDeps,
  existing: RuleDto | null,
  preselectItemId: string | null,
  onSaved: () => void
): void {
  const creating = existing === null;
  let dirty = false;
  const initialValue = existing === null ? defaultRuleFormValue() : ruleToFormValue(existing);
  if (creating && preselectItemId !== null) {
    // 深链预填(?item=):仅当商品属于当前账号,否则回落账号级
    initialValue.itemId = deps.items.some((i) => i.id === preselectItemId) ? preselectItemId : "";
    initialValue.priority = PRIORITY_DEFAULT;
  }

  const titleScope = creating ? "新建" : `编辑 · ${scopeLabel(existing, deps.refs).slice(0, 20)}`;
  const handle = openDrawer(opener, {
    title: `自动化规则 · ${titleScope}`,
    isDirty: () => dirty
  });
  handle.body.closest(".drawer")?.classList.add("drawer-wide");

  const saveError = el("div", { class: "field-error" }, "");
  const saveBtn = btn(creating ? "创建规则" : "保存(整体替换)", { variant: "primary" });
  const formHost = el("div", {});

  const form = renderRuleForm(formHost, {
    accountId,
    items: deps.items,
    pools: deps.pools,
    templates: deps.templates,
    value: initialValue,
    lockScope: !creating,
    onDirty: () => {
      dirty = true;
      saveBtn.disabled = form.validate().length > 0;
    }
  });
  // 初次校验:新建空表单(无内容)即禁用保存;编辑态固定文字规则须重输正文
  saveBtn.disabled = form.validate().length > 0;

  saveBtn.addEventListener("click", async () => {
    const errors = form.validate();
    if (errors.length > 0) {
      saveError.textContent = `保存前需修正 ${errors.length} 处问题`;
      return;
    }
    saveBtn.disabled = true;
    saveError.textContent = "";
    const payload = form.buildPayload(existing === null ? null : existing.version);
    try {
      if (creating) {
        await httpPost(`/api/v1/accounts/${accountId}/rules`, payload);
      } else {
        await httpPut(`/api/v1/accounts/${accountId}/rules/${existing.id}`, payload);
      }
      dirty = false;
      handle.close(true);
      onSaved();
    } catch (e) {
      saveError.textContent = `保存失败:${e instanceof Error ? e.message : String(e)}`;
      saveBtn.disabled = false;
    }
  });

  const replaceWarn =
    existing !== null &&
    existing.triggerType !== "review_missing_timeout" &&
    existing.contentSource === "fixed_text"
      ? el(
          "div",
          { class: "banner banner-warning" },
          el(
            "span",
            {},
            `⚠ 当前接口不回读既有正文:固定文字内容保存将整体替换(v${existing.version});请在表单中重新输入完整内容`
          )
        )
      : null;

  handle.body.append(...(replaceWarn === null ? [] : [replaceWarn]), formHost, saveError);
  handle.foot.append(saveBtn);
}

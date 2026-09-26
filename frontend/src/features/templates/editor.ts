/** 发货模板编辑抽屉(007 T026):模板名称 + 启用 + 有序消息列表(1..=10 条,
 *  实时字数/占位符预校验,与服务端同规则)+ 右侧"变量和占位符"指南卡。
 *  保存走 POST/PUT(带 expected_version);服务端 422/400 消息写入 field-error。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpPost, httpPut } from "../../shared/http";
import { errorHint, type FieldError } from "../../shared/contracts";
import { openDrawer, type DrawerHandle } from "../../ui/drawer";
import { btn } from "../../ui/dom";
import { icon } from "../../ui/icons";
import {
  type TemplateDto,
  MAX_MESSAGE_SCALARS,
  MAX_MESSAGES,
  VARIABLE_DOCS,
  VARIABLE_EXAMPLE,
  checkMessageBody,
  validateMessages,
  validateTemplateName
} from "./model";

export interface TemplateEditorCallbacks {
  onSaved: () => void;
}

interface MessageRow {
  area: HTMLTextAreaElement;
  err: HTMLElement;
  count: HTMLElement;
}

/** 已知错误码集(与 contracts ERROR_HINTS 收录范围一致;模板页仅 referenced_resource 相关)。 */
const ERROR_HINTED = new Set(["referenced_resource"]);

const apiErrorMessage = (e: unknown): string => {
  if (e instanceof ApiHttpError) {
    const code = e.body?.code;
    const hint = code !== undefined && ERROR_HINTED.has(code) ? errorHint(code, e.message) : "";
    return hint.length > 0 ? `${hint}(${e.message})` : e.message;
  }
  return e instanceof Error ? e.message : String(e);
};

/** 宽抽屉:左侧表单 + 右侧变量指南两栏(复用 .drawer,仅加宽 class)。 */
function widenDrawer(handle: DrawerHandle): void {
  handle.body.closest(".drawer")?.classList.add("drawer-wide");
}

export function openTemplateEditor(
  opener: HTMLElement | null,
  existing: TemplateDto | null,
  cb: TemplateEditorCallbacks
): void {
  let dirty = false;
  const creating = existing === null;
  const handle = openDrawer(opener, {
    title: creating ? "新建模板" : `编辑模板 · ${existing!.name.slice(0, 16)}`,
    isDirty: () => dirty
  });
  widenDrawer(handle);

  // ---- 通用字段(三件套:field + hint + field-error)----
  const nameInput = el("input", { type: "text", placeholder: "模板名(必填,≤100 字)" }) as HTMLInputElement;
  nameInput.value = existing?.name ?? "";
  const nameError = el("div", { class: "field-error" }, "");
  const enabledCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  enabledCheck.checked = existing?.enabled ?? true;
  const saveError = el("div", { class: "field-error" }, "");
  const saveBtn = btn(creating ? "创建" : "保存", { variant: "primary" });

  // ---- 消息列表(自上而下有序;输入原位更新,增删才重建行)----
  let messages: string[] = existing !== null ? existing.messages.slice() : [""];
  const rows: MessageRow[] = [];
  const listHost = el("div", { class: "tpl-msg-rows" });
  const addBtn = btn("+ 添加消息", { variant: "secondary" });

  const syncAddBtn = (): void => {
    if (messages.length >= MAX_MESSAGES) {
      addBtn.disabled = true;
      addBtn.title = `上限 ${MAX_MESSAGES} 条消息`;
    } else {
      addBtn.disabled = false;
      addBtn.title = "";
    }
  };

  /** 单行实时校验:空/占位符语法(带位置)/超限;字数实时显示(N/1000 标量)。 */
  const refreshRow = (i: number): boolean => {
    const row = rows[i];
    if (row === undefined) return true;
    const body = messages[i] ?? "";
    row.count.textContent = `${[...body].length}/${MAX_MESSAGE_SCALARS}`;
    const check = checkMessageBody(body, i + 1);
    row.err.textContent = check.ok ? "" : check.message;
    row.area.setAttribute("aria-invalid", check.ok ? "false" : "true");
    return check.ok;
  };

  const buildRow = (i: number): HTMLElement => {
    const area = el("textarea", {
      rows: "3",
      placeholder: "本条消息内容,支持 {{变量}} 占位符"
    }) as HTMLTextAreaElement;
    area.value = messages[i] ?? "";
    area.style.minHeight = "72px";
    const err = el("div", { class: "field-error" }, "");
    const count = el("span", { class: "tpl-msg-count" }, "0/1000");
    const delBtn = el("button", {
      class: "icon-btn danger-icon",
      type: "button",
      title: `删除第 ${i + 1} 条消息`
    });
    delBtn.append(icon("trash"));
    area.addEventListener("input", () => {
      dirty = true;
      messages[i] = area.value;
      refreshRow(i);
      refreshSaveGate();
    });
    delBtn.addEventListener("click", () => {
      // 至少保留一条:删最后一条按钮禁用,这里兜底
      if (messages.length <= 1) return;
      dirty = true;
      messages.splice(i, 1);
      rebuildRows();
    });
    rows[i] = { area, err, count };
    return el(
      "div",
      { class: "tpl-msg-row" },
      el(
        "div",
        { class: "tpl-msg-head" },
        el("span", { class: "tpl-msg-index" }, `消息 ${i + 1}`),
        count,
        delBtn
      ),
      area,
      err
    );
  };

  const rebuildRows = (): void => {
    rows.length = 0;
    listHost.replaceChildren(...messages.map((_, i) => buildRow(i)));
    // 删最后一条禁用(至少保留一条)
    if (rows.length === 1) {
      const last = listHost.querySelector<HTMLButtonElement>(".tpl-msg-row:last-child .icon-btn");
      if (last !== null) {
        last.disabled = true;
        last.title = "至少保留一条消息";
      }
    }
    for (let i = 0; i < rows.length; i++) refreshRow(i);
    syncAddBtn();
  };

  addBtn.addEventListener("click", () => {
    if (messages.length >= MAX_MESSAGES) return;
    dirty = true;
    messages.push("");
    rebuildRows();
    rows[messages.length - 1]?.area.focus();
  });

  // ---- 保存门禁:名称 + 全部消息通过才可保存(与 rule-editor 同模式)----
  const refreshSaveGate = (): void => {
    const name = validateTemplateName(nameInput.value);
    nameError.textContent = name.message;
    nameInput.setAttribute("aria-invalid", name.ok ? "false" : "true");
    let ok = name.ok;
    for (let i = 0; i < rows.length; i++) {
      if (!refreshRow(i)) ok = false;
    }
    if (!validateMessages(messages).ok) ok = false;
    saveBtn.disabled = !ok;
  };
  nameInput.addEventListener("input", () => {
    dirty = true;
    refreshSaveGate();
  });
  enabledCheck.addEventListener("change", () => {
    dirty = true;
  });

  // ---- 保存:POST 创建 / PUT 更新(乐观锁 expected_version)----
  saveBtn.addEventListener("click", async () => {
    refreshSaveGate();
    if (saveBtn.disabled) return;
    saveBtn.disabled = true;
    const payload = {
      name: nameInput.value.trim(),
      enabled: enabledCheck.checked,
      messages
    };
    try {
      if (creating) {
        await httpPost("/api/v1/delivery-templates", payload);
      } else {
        await httpPut(`/api/v1/delivery-templates/${existing!.id}`, {
          ...payload,
          expected_version: existing!.version
        });
      }
      dirty = false;
      handle.close(true);
      cb.onSaved();
    } catch (e) {
      // 服务端 422/400 校验消息写入对应 field-error:有 field_errors 按字段落位,
      // 否则统一落到保存错误行(信封 message 已含定位信息)。
      if (e instanceof ApiHttpError && Array.isArray(e.body?.field_errors)) {
        let rest: string[] = [];
        for (const fe of e.body?.field_errors ?? ([] as FieldError[])) {
          if (fe.field === "name") nameError.textContent = fe.message;
          else rest.push(fe.message);
        }
        saveError.textContent =
          rest.length > 0 ? `保存失败:${rest.join(";")}` : `保存失败:${e.message}`;
      } else {
        saveError.textContent = `保存失败:${apiErrorMessage(e)}`;
      }
      saveBtn.disabled = false;
    }
  });

  rebuildRows();
  refreshSaveGate();

  // ---- 右侧:变量和占位符指南卡(contracts §2 + data-model 语法)----
  const guideRows = VARIABLE_DOCS.map((d) =>
    el(
      "div",
      { class: "tpl-guide-row" },
      el("code", { class: "mono" }, d.token),
      el("span", { class: "tpl-guide-desc" }, d.desc)
    )
  );
  const guide = el(
    "aside",
    { class: "tpl-guide", "aria-label": "变量和占位符指南" },
    el("h3", { class: "tpl-guide-title" }, "变量和占位符"),
    el(
      "p",
      { class: "tpl-guide-note" },
      "发货时占位符按订单事实替换;变量名仅限字母/数字/下划线/连字符,首尾空格容错(如 ",
      el("code", { class: "mono" }, "{{ order_id }}"),
      " 合法)。"
    ),
    ...guideRows,
    el("div", { class: "tpl-guide-title", style: "margin-top:12px;" }, "示例"),
    el("pre", { class: "tpl-guide-example" }, VARIABLE_EXAMPLE),
    el("p", { class: "tpl-guide-note" }, "多消息模板按列表顺序逐条发送;预览与实际发送逐字一致(卡密内容掩码显示)。")
  );

  // ---- 装配:左表单 / 右指南两栏 ----
  const form = el(
    "div",
    { class: "tpl-edit-form" },
    el("div", { class: "field" }, el("label", {}, "模板名称"), nameInput, nameError),
    el("label", { class: "check-row" }, enabledCheck, "启用该模板"),
    el(
      "div",
      { class: "field" },
      el("label", {}, `消息列表(自上而下逐条发送,1—${MAX_MESSAGES} 条)`),
      el(
        "div",
        { class: "hint" },
        "每条消息独立发送;删除仅作用于单条,最后一条不可删。"
      ),
      listHost,
      el("div", { style: "display:flex;justify-content:flex-start;" }, addBtn)
    ),
    saveError
  );

  handle.body.append(el("div", { class: "tpl-edit-grid" }, form, guide));
  handle.foot.append(saveBtn);
}

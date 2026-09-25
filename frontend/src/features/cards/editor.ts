/** 卡密组编辑抽屉 + 批量导入弹窗(007 T016)。
 *  契约不变量:后端永不回显明文(卡密行/固定内容/headers/params/body 值),
 *  编辑态只显示"已配置"摘要;text/image 换内容=整体替换;data 组追加走 append-data。
 *  API 构建器内置"测试"按钮(POST /card-pools/test-api,不落库)。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpPost, httpPut } from "../../shared/http";
import { session } from "../../app/session";
import { type ApiErrorBody, type CardPoolDto, type CardPoolKind } from "../../shared/contracts";
import { openDrawer } from "../../ui/drawer";
import { openModal } from "../../ui/modal";
import { btn } from "../../ui/dom";
import {
  KIND_LABELS,
  buildApiConfig,
  countDataLines,
  dataLines,
  parseAppendResult,
  parseImportReport,
  parseTestApiResult,
  stockView,
  validateDelaySeconds,
  type ApiFormInput,
  type ImportReport
} from "./model";

const KIND_ORDER: CardPoolKind[] = ["data", "text", "image", "api"];

export interface CardPoolEditorCallbacks {
  onSaved: () => void;
}

/** 四类型编辑抽屉:existing=null 为创建;编辑态类型锁定(后端无改类型语义)。 */
export function openCardPoolEditor(
  opener: HTMLElement | null,
  existing: CardPoolDto | null,
  cb: CardPoolEditorCallbacks
): void {
  let dirty = false;
  const handle = openDrawer(opener, {
    title: existing === null ? "添加新卡密" : `编辑卡密组 · ${existing.name.slice(0, 16)}`,
    isDirty: () => dirty
  });

  let kind: CardPoolKind = existing?.kind ?? "data";
  const creating = existing === null;

  // ---- 通用字段(三件套:field + hint + field-error)----
  const nameInput = el("input", { type: "text", placeholder: "组名(必填,≤100 字)" }) as HTMLInputElement;
  nameInput.value = existing?.name ?? "";
  const nameError = el("div", { class: "field-error" }, "");
  const delayInput = el("input", {
    type: "number",
    min: "0",
    max: "3600",
    step: "1",
    value: String(existing?.delay_seconds ?? 0)
  }) as HTMLInputElement;
  const delayError = el("div", { class: "field-error" }, "");
  const descInput = el("textarea", {
    rows: "2",
    placeholder: "可选备注,便于区分用途(≤500 字)",
    style: "min-height:56px;"
  }) as HTMLTextAreaElement;
  descInput.value = existing?.description ?? "";
  const enabledCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  enabledCheck.checked = existing?.enabled ?? true;
  const saveError = el("div", { class: "field-error" }, "");
  const saveBtn = btn(creating ? "创建" : "保存", { variant: "primary" });

  // ---- 类型选择(卡片按钮;编辑态锁定)----
  const kindBox = el("div", { class: "range-chips", style: "margin-bottom:0;" });
  const kindBtns = new Map<CardPoolKind, HTMLButtonElement>();
  const syncKindActive = (): void => {
    for (const [k, b] of kindBtns) {
      const active = k === kind;
      b.classList.toggle("active", active);
      b.setAttribute("aria-pressed", active ? "true" : "false");
    }
  };
  for (const k of KIND_ORDER) {
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

  // ---- data:批量库存(创建=初始条目;编辑=追加,不回显明文)----
  const stockNote =
    existing !== null && existing.kind === "data"
      ? (() => {
          const s = stockView(existing.stock);
          return el(
            "div",
            { class: "hint" },
            s === null
              ? "已有库存计数不可用;追加不影响已有卡密"
              : `已有库存:${s.total} 条(可用 ${s.available} · 预留 ${s.reserved} · 已用 ${s.used});已有卡密不回显,追加不影响`
          );
        })()
      : el("div", { class: "hint" }, "一行一条卡密;空行自动忽略,重复行自动去重");
  const dataArea = el("textarea", {
    placeholder: creating ? "一行一条卡密,如:\nCARD-0001\nCARD-0002" : "追加新卡密,一行一条;留空表示本次不追加"
  }) as HTMLTextAreaElement;
  const dataCount = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "0 条");
  dataArea.addEventListener("input", () => {
    dirty = true;
    dataCount.textContent = `${countDataLines(dataArea.value)} 条`;
  });

  // ---- text / image:固定内容(编辑不回显,输入新值=整体替换)----
  const contentSet = existing?.content_set === true;
  const baseContentPlaceholder =
    kind === "image" ? "https://…(买家将收到的图片直链)" : "买家将收到的固定内容,如:链接 + 提取码";
  const contentArea = el("textarea", {
    placeholder: creating || !contentSet ? baseContentPlaceholder : "已配置(不回显);输入新内容将整体替换"
  }) as HTMLTextAreaElement;
  const contentError = el("div", { class: "field-error" }, "");
  const contentLabelEl = el("label", {}, "");
  const contentLabel = (): string => (kind === "image" ? "图片链接" : "固定内容");

  // ---- api:内嵌构建器 + 一次性测试 ----
  const api = existing?.api_config ?? null;
  let apiDirty = false;
  const markApiDirty = (): void => {
    apiDirty = true;
    dirty = true;
  };
  const apiUrl = el("input", { type: "text", placeholder: "https://api.example.com/fetch-card" }) as HTMLInputElement;
  apiUrl.value = api?.url ?? "";
  let apiMethod: "GET" | "POST" = api?.method.toUpperCase() === "POST" ? "POST" : "GET";
  const methodBox = el("div", { class: "range-chips", style: "margin-bottom:0;" });
  const methodBtns = new Map<"GET" | "POST", HTMLButtonElement>();
  const syncMethodActive = (): void => {
    for (const [m, b] of methodBtns) {
      const active = m === apiMethod;
      b.classList.toggle("active", active);
      b.setAttribute("aria-pressed", active ? "true" : "false");
    }
  };
  for (const m of ["GET", "POST"] as const) {
    const b = el("button", { class: "chip", type: "button", "aria-pressed": "false" }, m) as HTMLButtonElement;
    b.addEventListener("click", () => {
      apiMethod = m;
      markApiDirty();
      syncMethodActive();
      refreshValidation();
    });
    methodBtns.set(m, b);
    methodBox.append(b);
  }
  syncMethodActive();
  const maskedNote = (label: string, configured: boolean): string =>
    creating || !configured
      ? `JSON 对象,如 {"Authorization":"Bearer …"}(可选)`
      : `${label}已配置(不回显);输入新内容将整体替换,留空视为清除`;
  const timeoutInput = el("input", {
    type: "number",
    min: "1",
    max: "60",
    step: "1",
    value: String(api !== null ? Math.round(api.timeout_ms / 1000) : 10)
  }) as HTMLInputElement;
  const headersArea = el("textarea", {
    placeholder: maskedNote("请求头", api?.headers_configured === true),
    style: "min-height:64px;"
  }) as HTMLTextAreaElement;
  const paramsArea = el("textarea", {
    placeholder: maskedNote("参数", api?.params_configured === true),
    style: "min-height:64px;"
  }) as HTMLTextAreaElement;
  const contentTypeInput = el("input", { type: "text", placeholder: "application/json(可选)" }) as HTMLInputElement;
  contentTypeInput.value = api?.content_type ?? "";
  const bodyArea = el("textarea", {
    placeholder:
      creating || api?.body_configured !== true
        ? '{"order_no":"…"}(POST 请求体,可选)'
        : "请求体已配置(不回显);输入新内容将整体替换,留空视为清除",
    style: "min-height:64px;"
  }) as HTMLTextAreaElement;
  const pathInput = el("input", { type: "text", placeholder: "如 data.card(点号取值路径)" }) as HTMLInputElement;
  pathInput.value = api?.response_path ?? "";
  const retryCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  retryCheck.checked = api?.retry_enabled ?? false;
  const apiError = el("div", { class: "field-error" }, "");
  const testBtn = btn("测试", { variant: "secondary" });
  const testOut = el("pre", {
    class: "preview",
    style: "display:none;white-space:pre-wrap;word-break:break-all;border:1px dashed var(--border-strong);padding:8px;border-radius:6px;margin:0;font-size:var(--text-xs);"
  });

  const collectApiForm = (): ApiFormInput => ({
    url: apiUrl.value,
    method: apiMethod,
    timeoutSeconds: timeoutInput.value,
    headersJson: headersArea.value,
    paramsJson: paramsArea.value,
    contentType: contentTypeInput.value,
    body: bodyArea.value,
    responsePath: pathInput.value,
    retryEnabled: retryCheck.checked
  });

  // ---- 按类型显隐的分区容器 ----
  const dataFields = el(
    "div",
    {},
    el(
      "div",
      { class: "field" },
      el("label", {}, "卡密条目(一行一条)"),
      dataArea,
      el("div", { style: "display:flex;justify-content:flex-end;" }, dataCount),
      stockNote
    )
  );
  const textFields = el(
    "div",
    {},
    el("div", { class: "field" }, contentLabelEl, contentArea, contentError)
  );
  const apiFields = el(
    "div",
    {},
    el("div", { class: "field" }, el("label", {}, "API 地址"), apiUrl),
    el("div", { class: "field" }, el("label", {}, "请求方法"), methodBox),
    el("div", { class: "field" }, el("label", {}, "超时(秒,1—60)"), timeoutInput),
    el("div", { class: "field" }, el("label", {}, "请求头(JSON)"), headersArea),
    el("div", { class: "field" }, el("label", {}, "参数(JSON)"), paramsArea),
    el("div", { class: "field" }, el("label", {}, "Content-Type"), contentTypeInput),
    el("div", { class: "field" }, el("label", {}, "请求体(JSON,POST)"), bodyArea),
    el("div", { class: "field" }, el("label", {}, "响应取值路径"), pathInput),
    el("label", { class: "check-row" }, retryCheck, "失败自动重试"),
    el(
      "div",
      { style: "display:flex;align-items:center;gap:12px;margin-bottom:8px;" },
      testBtn,
      el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "用当前表单发起一次真实取卡,不落库")
    ),
    testOut,
    apiError
  );
  dataFields.hidden = true;
  textFields.hidden = true;
  apiFields.hidden = true;
  const syncKindFields = (): void => {
    dataFields.hidden = kind !== "data";
    textFields.hidden = !(kind === "text" || kind === "image");
    apiFields.hidden = kind !== "api";
    contentLabelEl.textContent = contentLabel();
  };
  syncKindFields();

  // ---- 行内实时校验(与 rule-editor 同模式:三件套 + saveBtn 门禁)----
  const refreshValidation = (): void => {
    let ok = true;
    if (nameInput.value.trim().length === 0) {
      nameError.textContent = "名称不能为空";
      ok = false;
    } else if ([...nameInput.value.trim()].length > 100) {
      nameError.textContent = "名称过长(上限 100 字)";
      ok = false;
    } else {
      nameError.textContent = "";
    }
    nameInput.setAttribute("aria-invalid", nameError.textContent === "" ? "false" : "true");

    const d = validateDelaySeconds(delayInput.value);
    delayError.textContent = d.message;
    delayInput.setAttribute("aria-invalid", d.ok ? "false" : "true");
    if (!d.ok) ok = false;

    if (kind === "text" || kind === "image") {
      const v = contentArea.value.trim();
      if (creating && v.length === 0) {
        contentError.textContent = "内容不能为空";
        ok = false;
      } else if ([...v].length > 1000) {
        contentError.textContent = "超出字数上限(1000 字符)";
        ok = false;
      } else {
        contentError.textContent = "";
      }
      contentArea.setAttribute("aria-invalid", contentError.textContent === "" ? "false" : "true");
    }

    if (kind === "api") {
      const built = buildApiConfig(collectApiForm());
      apiError.textContent = built.ok ? "" : built.error;
      if (!built.ok) ok = false;
    }
    saveBtn.disabled = !ok;
  };
  nameInput.addEventListener("input", () => {
    dirty = true;
    refreshValidation();
  });
  delayInput.addEventListener("input", () => {
    dirty = true;
    refreshValidation();
  });
  descInput.addEventListener("input", () => {
    dirty = true;
  });
  enabledCheck.addEventListener("change", () => {
    dirty = true;
  });
  contentArea.addEventListener("input", () => {
    dirty = true;
    refreshValidation();
  });
  for (const node of [apiUrl, timeoutInput, headersArea, paramsArea, contentTypeInput, bodyArea, pathInput]) {
    node.addEventListener("input", () => {
      markApiDirty();
      refreshValidation();
    });
  }
  retryCheck.addEventListener("change", () => {
    markApiDirty();
    refreshValidation();
  });
  refreshValidation();

  // ---- 测试按钮:POST /card-pools/test-api(不落库)----
  testBtn.addEventListener("click", async () => {
    const built = buildApiConfig(collectApiForm());
    testOut.style.display = "block";
    if (!built.ok) {
      testOut.replaceChildren(el("span", { class: "error-text" }, `✗ 配置不完整:${built.error}`));
      return;
    }
    testBtn.disabled = true;
    try {
      const res = parseTestApiResult(await httpPost("/api/v1/card-pools/test-api", built.config));
      if (res.ok) {
        testOut.replaceChildren(
          el("div", { class: "tone-normal", style: "margin-bottom:4px;" }, `✓ 取卡成功 · 延迟 ${res.latency_ms ?? "—"} ms`),
          el("span", {}, `样例值:${res.sample ?? "—"}`)
        );
      } else {
        testOut.replaceChildren(
          el("div", { class: "error-text", style: "margin-bottom:4px;" }, `✗ 取卡失败${res.latency_ms !== null ? ` · 延迟 ${res.latency_ms} ms` : ""}`),
          el("span", { class: "error-text" }, res.error ?? "未知错误")
        );
      }
    } catch (e) {
      testOut.replaceChildren(
        el("span", { class: "error-text" }, `测试请求失败:${e instanceof Error ? e.message : String(e)}`)
      );
    } finally {
      testBtn.disabled = false;
    }
  });

  // ---- 保存:create / update(乐观锁)+ data 组编辑态追加 ----
  saveBtn.addEventListener("click", async () => {
    refreshValidation();
    if (saveBtn.disabled) return;
    saveBtn.disabled = true;
    const d = validateDelaySeconds(delayInput.value);
    const body: Record<string, unknown> = {
      name: nameInput.value.trim(),
      kind,
      enabled: enabledCheck.checked,
      delay_seconds: d.value,
      description: descInput.value.trim(),
      entries: []
    };
    if (kind === "data" && creating) {
      body["entries"] = dataLines(dataArea.value);
    }
    if (kind === "text" || kind === "image") {
      const content = contentArea.value.trim();
      // 编辑态留空 = 不替换既有内容;创建态必填已由校验拦截
      if (content.length > 0) body["content"] = content;
    }
    if (kind === "api") {
      const built = buildApiConfig(collectApiForm());
      if (!built.ok) {
        apiError.textContent = built.error;
        saveBtn.disabled = false;
        return;
      }
      // 编辑态未触碰 API 区不重发配置(避免用脱敏表单清掉已配置的 headers 等值)
      if (creating || apiDirty) body["api_config"] = built.config;
    }
    try {
      let appendNotice: { appended: number; skipped_duplicate: number } | null = null;
      if (creating) {
        await httpPost("/api/v1/card-pools", body);
      } else {
        await httpPut(`/api/v1/card-pools/${existing!.id}`, { ...body, expected_version: existing!.version });
        if (kind === "data") {
          const lines = dataLines(dataArea.value);
          if (lines.length > 0) {
            const appended = parseAppendResult(
              await httpPost(`/api/v1/card-pools/${existing!.id}/append-data`, { lines })
            );
            // 同名重复行被跳过:如实告知,不静默吞掉
            if (appended.skipped_duplicate > 0) appendNotice = appended;
          }
        }
      }
      dirty = false;
      handle.close(true);
      cb.onSaved();
      if (appendNotice !== null) showAppendNotice(appendNotice);
    } catch (e) {
      saveError.textContent = `保存失败:${e instanceof Error ? e.message : String(e)}`;
      saveBtn.disabled = false;
    }
  });

  const field = (label: string, control: HTMLElement, ...hints: HTMLElement[]): HTMLElement =>
    el("div", { class: "field" }, el("label", {}, label), control, ...hints);

  handle.body.append(
    field("名称", nameInput, nameError),
    field(
      "类型",
      kindBox,
      el("div", { class: "hint" }, creating ? "创建后类型不可更改" : "类型已锁定,不可更改")
    ),
    dataFields,
    textFields,
    apiFields,
    field("延时发货(秒,0—3600)", delayInput, el("div", { class: "hint" }, "付款后延迟 N 秒再发货"), delayError),
    field("描述", descInput),
    el("label", { class: "check-row" }, enabledCheck, "启用该组"),
    saveError
  );
  handle.foot.append(saveBtn);
}

function showAppendNotice(result: { appended: number; skipped_duplicate: number }): void {
  // 轻量告知(不复用确认流的提交语义):追加完成但存在重复行
  const box = el("div", { class: "banner banner-warning", role: "status" },
    el("strong", {}, "追加完成"),
    el("span", {}, `新增 ${result.appended} 条;${result.skipped_duplicate} 条重复被跳过`));
  document.querySelector(".app-main")?.prepend(box);
  window.setTimeout(() => box.remove(), 5000);
}

// ===== 批量导入弹窗(模板下载 / multipart 上传 / 逐行结果)=====

/** 模板列(与后端 REQUIRED_HEADERS/OPTIONAL_HEADERS 一致,顺序固定)。 */
export const IMPORT_TEMPLATE_HEADERS = "名称,类型,内容,描述,启用,延迟秒";

function downloadTemplate(): void {
  // UTF-8 BOM:Excel 直接双击打开中文不乱码
  const csv = `\uFEFF${IMPORT_TEMPLATE_HEADERS}\n`;
  const blob = new Blob([csv], { type: "text/csv;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = el("a", { href: url, download: "卡密批量导入模板.csv" });
  document.body.append(a);
  a.click();
  a.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** multipart 上传(http.ts 只发 JSON,此处按同约定手发表单:CSRF + 幂等键 + 错误信封)。 */
async function uploadImportFile(file: File): Promise<ImportReport> {
  const fd = new FormData();
  fd.append("file", file, file.name);
  const res = await fetch("/api/v1/card-pools/batch-import", {
    method: "POST",
    headers: { "X-CSRF-Token": session.csrfToken, "X-Idempotency-Key": crypto.randomUUID() },
    body: fd,
    credentials: "same-origin"
  });
  if ((res.status === 401 || res.status === 403) && session.authenticated) {
    document.dispatchEvent(new CustomEvent("qing:session-expired"));
  }
  const text = await res.text();
  const payload: unknown = text.length > 0 ? JSON.parse(text) : null;
  if (!res.ok) {
    const body = payload as Record<string, unknown> | null;
    throw new ApiHttpError(res.status, (body?.["error"] as ApiErrorBody | undefined) ?? null);
  }
  return parseImportReport(payload);
}

export interface ImportModalCallbacks {
  onImported: () => void;
}

export function openImportModal(opener: HTMLElement | null, cb: ImportModalCallbacks): void {
  const handle = openModal(opener, { title: "批量导入卡密" });
  const tplBtn = btn("下载 CSV 模板", { variant: "secondary" });
  const fileInput = el("input", { type: "file", accept: ".xlsx,.xls,.csv,.tsv" }) as HTMLInputElement;
  fileInput.style.width = "auto";
  const uploadBtn = btn("上传并导入", { variant: "primary" });
  const errLine = el("div", { class: "field-error" }, "");
  const resultBox = el("div", {});
  let imported = false;

  tplBtn.addEventListener("click", downloadTemplate);
  fileInput.addEventListener("change", () => {
    errLine.textContent = "";
    uploadBtn.disabled = fileInput.files === null || fileInput.files.length === 0;
  });
  uploadBtn.disabled = true;
  uploadBtn.addEventListener("click", async () => {
    const file = fileInput.files?.[0];
    if (file === undefined) return;
    uploadBtn.disabled = true;
    uploadBtn.textContent = "导入中…";
    try {
      const report = await uploadImportFile(file);
      imported = imported || report.succeeded > 0;
      renderReport(report);
      if (report.succeeded > 0) cb.onImported();
    } catch (e) {
      errLine.textContent = `导入失败:${e instanceof Error ? e.message : String(e)}`;
    } finally {
      uploadBtn.textContent = "上传并导入";
      uploadBtn.disabled = false;
    }
  });

  const renderReport = (report: ImportReport): void => {
    const failedRows = report.failed.map((f) =>
      el("li", { style: "margin:0;" }, el("span", { class: "mono" }, `第 ${f.row} 行`), `:${f.error}`)
    );
    resultBox.replaceChildren(
      el(
        "div",
        { class: `banner ${report.failed.length > 0 ? "banner-warning" : "banner"}`, style: report.failed.length > 0 ? "" : "border:1px solid var(--tone-normal-fg);color:var(--tone-normal-fg);background:var(--tone-normal-bg);" },
        el("strong", {}, "导入完成"),
        el("span", {}, `共 ${report.total} 行:成功 ${report.succeeded} · 失败 ${report.failed.length};仅成功行入库`)
      ),
      ...(failedRows.length > 0
        ? [
            el(
              "ul",
              { class: "timeline", style: "margin:8px 0 0;padding-left:4px;" },
              ...failedRows
            )
          ]
        : [])
    );
  };

  const closeBtn = btn("关闭", { variant: "secondary" });
  closeBtn.addEventListener("click", () => handle.close(true));

  handle.body.append(
    el(
      "p",
      { class: "muted", style: "font-size:var(--text-xs);margin:0 0 12px;" },
      "支持 .xlsx / .csv / .tsv(≤2 MiB);一行 = 一个卡密组(批量组一行一条卡密,或文本/图片单行组);API 组请在页面单独配置"
    ),
    el("div", { style: "display:flex;gap:12px;align-items:center;flex-wrap:wrap;" }, tplBtn, fileInput, uploadBtn),
    errLine,
    resultBox,
    el("div", { style: "display:flex;gap:12px;justify-content:flex-end;margin-top:16px;" }, closeBtn)
  );
}

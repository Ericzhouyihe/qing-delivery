/** 账号管理页(006):Ydisks 参考图复刻——页头/搜索/计数横幅/条目/图标操作组/视图切换。
 *  "算"在 model.ts(纯函数),本文件只装配与调用既有 API(宪章 II);
 *  既有确认流与轮询行为零回退(FR-012)。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpGet, httpPost, httpRequest, httpPatch } from "../../shared/http";
import { badgeEl, accountBadge } from "../../ui/badge";
import { btn } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { formatRelative, startTicker } from "../../ui/time";
import { confirmDialog, openModal } from "../../ui/modal";
import { icon, type IconName } from "../../ui/icons";
import { createSegmented } from "../../ui/segmented";
import {
  deriveCapabilityTags,
  filterAccounts,
  parseAccountCard,
  readViewPreference,
  sortAccounts,
  viewCounts,
  writeViewPreference,
  type AccountCard,
  type AccountViewMode
} from "./model";
import { openQrModal } from "./qr-modal";
import type { PageFactory } from "../../app/page";

export { parseAccountCard };
export type { AccountCard };

export const accountsPage: PageFactory = (root) => {
  // 视图模式(横条/卡片):偏好读写经 model(US3/FR-010),未知旧值回退 "row"
  let viewMode: AccountViewMode = readViewPreference();
  let query = "";
  let cache: AccountCard[] | null = null;
  let inflight = 0;
  const timeCells: Array<[HTMLElement, string]> = [];
  let stopTicker: (() => void) | null = null;
  /** 控制屏障进行中的账号(US3-4):仅由 applyControl 显式标记,不从状态推断。 */
  const pausingIds = new Set<string>();

  /** 计数横幅(006 FR-004):常驻,X=过滤后可见数,Y=总数。 */
  const countEl = el("span", { class: "count-banner-count" });
  const banner = el(
    "div",
    { class: "count-banner" },
    countEl,
    el(
      "span",
      { class: "count-banner-hint" },
      "如果某个账号只显示 ID，点该账号右侧“刷新资料”；若刷新失败，先点二维码重新授权。"
    )
  );
  const syncBanner = (): void => {
    const total = cache?.length ?? 0;
    const shown = cache === null ? 0 : filterAccounts(cache, query).length;
    const c = viewCounts(shown, total);
    countEl.textContent = `当前显示 ${c.shown} / ${c.total} 个账号`;
  };

  const restartTicker = (): void => {
    stopTicker?.();
    stopTicker = startTicker(() => {
      for (const [cell, iso] of timeCells) cell.textContent = formatRelative(iso);
    }, 30_000);
  };

  /** 搜索无匹配轻空态(006 R8):与总数为 0 的引导空态区分,提供一步清空。 */
  const noMatchEl = (): HTMLElement => {
    const clear = btn("清空搜索", { variant: "secondary" });
    clear.addEventListener("click", () => {
      searchInput.value = "";
      query = "";
      rerenderLocal();
    });
    return el(
      "div",
      { class: "no-match" },
      el("div", { class: "empty-ico" }, "🔍"),
      el("div", {}, "无匹配账号"),
      el("div", { class: "muted" }, "试试其他昵称、备注或账号 ID 关键词"),
      clear
    );
  };

  /** 由 cache 构建列表容器;返回 null 表示走 asyncBlock 空态(账号总数为 0)。 */
  const buildGrid = (): HTMLElement | null => {
    if (cache === null || cache.length === 0) return null;
    const items = filterAccounts(cache, query);
    syncBanner();
    if (items.length === 0) return noMatchEl();
    timeCells.length = 0;
    const wrap = el("div", { class: viewMode === "card" ? "account-grid" : "account-rows" });
    for (const a of items) wrap.append(viewMode === "card" ? renderCard(a) : renderRow(a));
    restartTicker();
    return wrap;
  };

  const load = async (): Promise<HTMLElement | null> => {
    inflight++;
    try {
      const raw = await httpGet("/api/v1/accounts");
      const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
      cache = sortAccounts(items.map(parseAccountCard));
      syncBanner();
      return buildGrid();
    } finally {
      inflight--;
    }
  };

  /** 本地即时过滤(006 FR-003):不重新请求,仅当列表/无匹配块在场时原位重建。 */
  const rerenderLocal = (): void => {
    syncBanner();
    if (inflight > 0) return;
    const slot = listHost.querySelector(":scope > .async-slot");
    if (slot === null) return;
    const current = slot.firstElementChild;
    if (current === null) return;
    const cls = current.classList;
    if (cls.contains("account-rows") || cls.contains("account-grid") || cls.contains("no-match")) {
      const node = buildGrid();
      if (node !== null) slot.replaceChildren(node);
    }
  };

  // ===== 006 共享装配(两形态一致,契约 §2.3)=====

  const needsAuth = (a: AccountCard): boolean =>
    a.connectionState === "authorization_expired" || a.connectionState === "verification_required";

  /** 徽标对:连接状态徽章 + 控制屏障过渡态(暂停中);sync 供 applyControl 刷新。 */
  const badgePairEl = (a: AccountCard): { box: HTMLElement; sync: () => void } => {
    const box = el("span", { class: "badge-pair" });
    const sync = (): void => {
      box.replaceChildren(badgeEl(accountBadge(a.connectionState), a.connectionState));
      if (pausingIds.has(a.id)) {
        box.append(el("span", { class: "badge badge-info" }, "暂停中"));
      }
    };
    sync();
    return { box, sync };
  };

  /** 能力徽标片段(006 FR-008):deriveCapabilityTags 输出,只映射真实能力。 */
  const tagsFrag = (a: AccountCard): DocumentFragment => {
    const frag = document.createDocumentFragment();
    for (const t of deriveCapabilityTags(a)) {
      frag.append(el("span", { class: `badge badge-${t.tone} badge-cap` }, t.label));
    }
    return frag;
  };

  /** 头像 + 右下角状态圆点:在线绿/异常红/其余灰;破图回退昵称首字。 */
  const avatarEl = (a: AccountCard): HTMLElement => {
    const box = el("div", { class: "avatar-box" });
    const dotCls = a.connectionState === "online" ? "dot-online" : needsAuth(a) ? "dot-danger" : "dot-offline";
    const dot = el("span", { class: `avatar-dot ${dotCls}` });
    if (a.avatarUrl !== null && a.avatarUrl.length > 0) {
      const img = el("img", { class: "avatar avatar-img", src: a.avatarUrl, alt: "头像" }) as HTMLImageElement;
      img.addEventListener("error", () => img.replaceWith(el("span", { class: "avatar" }, a.displayName.slice(0, 1) || "?")));
      box.append(img, dot);
    } else {
      box.append(el("span", { class: "avatar" }, a.displayName.slice(0, 1) || "?"), dot);
    }
    return box;
  };

  /** 纯图标按钮(006 契约 §2.3):内联 SVG + title 悬停提示,悬停变色由样式控制。 */
  const iconBtnEl = (name: IconName, title: string, cls: string): HTMLButtonElement => {
    const b = el("button", { class: `icon-btn ${cls}`, type: "button", title }) as HTMLButtonElement;
    b.append(icon(name));
    return b;
  };

  /** 验证会话徽章(003 逻辑,统一两形态):活跃会话 5s 轮询,离场经 MutationObserver 清理。 */
  const attachVerifyBadge = (a: AccountCard, badge: HTMLElement): (() => Promise<boolean>) => {
    const sync = async (): Promise<boolean> => {
      try {
        const res = (await httpGet(`/api/v1/accounts/${a.id}/verification`)) as Record<string, unknown>;
        const active = res["active"] as Record<string, unknown> | null;
        if (active === null || active === undefined) {
          const attempts = (res["recent_attempts"] ?? []) as Array<Record<string, unknown>>;
          const last = attempts[0];
          if (last !== undefined && last["outcome"] === "failed_manual") {
            badge.className = "badge badge-warning";
            badge.textContent = "待人工验证";
            return true;
          }
          badge.className = "badge badge-neutral";
          badge.textContent = "";
          return false;
        }
        const stateMap: Record<string, [string, string]> = {
          detected: ["badge-info", "验证中·已检测"],
          browser_open: ["badge-info", "验证中·浏览器已打开"],
          solving: ["badge-info", "验证中·滑块处理中"],
          updating_credential: ["badge-info", "验证中·凭证更新中"],
          succeeded: ["badge-normal", "验证成功"],
          failed_manual: ["badge-warning", "待人工验证"]
        };
        const [cls, text] = stateMap[String(active["state"] ?? "")] ?? ["badge-neutral", "验证中"];
        badge.className = `badge ${cls}`;
        badge.textContent = text;
        return true;
      } catch {
        return false;
      }
    };
    void (async () => {
      const active = await sync();
      if (!active) return;
      const timer = window.setInterval(async () => {
        const still = await sync();
        if (!still) window.clearInterval(timer);
      }, 5000);
      const obs = new MutationObserver(() => {
        if (!badge.isConnected) {
          window.clearInterval(timer);
          obs.disconnect();
        }
      });
      obs.observe(document.body, { childList: true, subtree: true });
    })();
    return sync;
  };

  const refreshAction = (a: AccountCard): HTMLButtonElement => {
    const b = iconBtnEl("refresh", "刷新昵称和头像", "");
    b.addEventListener("click", async () => {
      b.disabled = true;
      b.classList.add("spinning");
      try {
        await httpPost(`/api/v1/accounts/${a.id}/profile-fetch`, {});
      } catch {
        /* 刷新失败如实呈现,不打断其他操作 */
      } finally {
        b.disabled = false;
        b.classList.remove("spinning");
        blockRef?.reload();
      }
    });
    return b;
  };

  const reauthAction = (): HTMLButtonElement => {
    const b = iconBtnEl("qr", "重新授权", "reauth-icon");
    b.addEventListener("click", () => {
      openQrModal(b, { onAuthorized: () => blockRef?.reload() });
    });
    return b;
  };

  const verifyAction = (a: AccountCard, syncVerify: () => Promise<boolean>): HTMLButtonElement => {
    const b = iconBtnEl("shield", "发起安全验证", "");
    b.addEventListener("click", async () => {
      b.disabled = true;
      try {
        await httpPost(`/api/v1/accounts/${a.id}/verification`, {});
        confirmDialog({
          title: "安全验证已发起",
          message: "正在通过本机浏览器自动完成验证;完成后状态徽章自动更新。",
          confirmLabel: "知道了"
        });
      } catch (e) {
        // 409 = 已有活跃会话(契约返回既有 session)→ 继续轮询呈现而非报错
        if (e instanceof ApiHttpError && e.status === 409) {
          confirmDialog({
            title: "验证进行中",
            message: "该账号已有验证会话;完成后状态徽章自动更新。",
            confirmLabel: "知道了"
          });
          void syncVerify();
        } else {
          confirmDialog({
            title: "验证不可用",
            message: `发起验证失败:${e instanceof Error ? e.message : String(e)}`,
            confirmLabel: "关闭"
          });
        }
      } finally {
        b.disabled = false;
      }
    });
    return b;
  };

  const editAction = (a: AccountCard): HTMLButtonElement => {
    const b = iconBtnEl("edit", "编辑账号", "");
    b.addEventListener("click", () => openEditModal(a));
    return b;
  };

  const powerAction = (a: AccountCard, syncBadge: () => void): HTMLButtonElement => {
    const b = iconBtnEl("power", a.runEnabled ? "停用账号" : "启用账号", a.runEnabled ? "power-on" : "power-off");
    b.addEventListener("click", () => {
      const enabling = !a.runEnabled;
      confirmDialog({
        title: enabling ? "启用账号" : "停用账号",
        message: enabling
          ? "启用后该账号将恢复接单与自动发货(按其自动化开关)。"
          : "停用后该账号停止接单与发货;进行中的交付按暂停屏障安全收尾。",
        confirmLabel: enabling ? "启用" : "停用",
        danger: !enabling,
        onConfirm: () => applyControl(a, enabling, syncBadge)
      });
    });
    return b;
  };

  const deleteAction = (a: AccountCard): HTMLButtonElement => {
    const b = iconBtnEl("trash", `删除账号 ${a.displayName || a.platformUserId}`, "danger-icon");
    b.addEventListener("click", () => openDeleteDialog(a));
    return b;
  };

  /** 操作组(006 契约 §2.3):两形态一致;需重新授权的账号附加强调 qr 入口。 */
  const actionsEl = (a: AccountCard, syncBadge: () => void, syncVerify: () => Promise<boolean>): HTMLElement => {
    const actions = el("div", { class: "account-row-actions" });
    actions.append(refreshAction(a));
    if (needsAuth(a)) actions.append(reauthAction());
    actions.append(verifyAction(a, syncVerify), editAction(a), powerAction(a, syncBadge), deleteAction(a));
    return actions;
  };

  /** 行内警示条 + 重新授权(006 FR-006):需验证/授权失效账号。 */
  const alertEl = (a: AccountCard): HTMLElement | null => {
    if (!needsAuth(a)) return null;
    const reauth = btn("重新授权", { variant: "danger" });
    reauth.prepend(icon("qr", 14));
    reauth.classList.add("btn-with-icon", "reauth-inline");
    reauth.addEventListener("click", () => {
      openQrModal(reauth, { onAuthorized: () => blockRef?.reload() });
    });
    const msg =
      a.connectionState === "authorization_expired"
        ? "授权已失效，请重新扫码并完成授权"
        : "闲鱼要求安全验证，请重新扫码并完成验证";
    return el("div", { class: "account-row-alert" }, el("span", { class: "error-text" }, msg), reauth);
  };

  /** 条目主体(两形态共用):首行昵称+徽标,次行备注/ID/心跳,异常时警示条。
   *  extraLine2:形态差异项(卡片形态追加 ID 复制按钮)。 */
  const entryMainEl = (
    a: AccountCard,
    badgeBox: HTMLElement,
    verifyBadge: HTMLElement,
    extraLine2: HTMLElement[] = []
  ): HTMLElement => {
    const heartbeat = el("span", { class: "muted" }, a.monitoringSince ? formatRelative(a.monitoringSince) : "未启用监控");
    // 无监控起点时保持静态文案,不进 ticker(否则 30s 后被 formatRelative("") 清空)
    if (a.monitoringSince !== null) timeCells.push([heartbeat, a.monitoringSince]);
    const line2 = el(
      "div",
      { class: "account-row-line2 muted" },
      el("span", {}, a.remark !== null && a.remark.length > 0 ? a.remark : "暂无备注"),
      el("span", { class: "mono" }, `ID: ${a.platformUserId}`),
      ...extraLine2,
      el("span", {}, "心跳:"),
      heartbeat
    );
    const main = el(
      "div",
      { class: "account-row-main" },
      el(
        "div",
        { class: "account-row-line1" },
        el("strong", {}, a.displayName || "未命名账号"),
        badgeBox,
        tagsFrag(a),
        verifyBadge
      ),
      line2
    );
    const alert = alertEl(a);
    if (alert !== null) main.append(alert);
    return main;
  };

  /** 006:列表横条(Ydisks AccountCard 形态)。 */
  const renderRow = (a: AccountCard): HTMLElement => {
    const { box: badgeBox, sync: syncBadge } = badgePairEl(a);
    const verifyBadge = el("span", { class: "badge badge-neutral" }, "");
    const syncVerify = attachVerifyBadge(a, verifyBadge);
    const main = entryMainEl(a, badgeBox, verifyBadge);
    return el("div", { class: "account-row" }, avatarEl(a), main, actionsEl(a, syncBadge, syncVerify));
  };

  /** 004:账号编辑弹窗(对齐 Ydisks AccountEditModal)。 */
  const openEditModal = (a: AccountCard): void => {
    const handle = openModal(null, { title: "编辑账号" });
    const remarkInput = el("input", { type: "text", value: a.remark ?? "", placeholder: "为账号添加备注(≤64 字符)" }) as HTMLInputElement;
    const hint = el("div", { class: "field-error" }, "");
    const save = btn("保存", { variant: "primary" });
    const cancel = btn("取消", { variant: "secondary" });
    let autoDelivery = a.autoDelivery;
    let autoConfirm = a.autoConfirm;
    const mkSwitch = (label: string, checked: boolean, onChange: (v: boolean) => void): HTMLElement => {
      const sw = el("input", { type: "checkbox", role: "switch" }) as HTMLInputElement;
      sw.checked = checked;
      sw.addEventListener("change", () => onChange(sw.checked));
      return el("label", { class: "check-row" }, sw, label);
    };
    save.addEventListener("click", async () => {
      const v = remarkInput.value.trim();
      if (v.length > 64) { hint.textContent = "备注不得超过 64 字符"; return; }
      save.disabled = true;
      try {
        await httpPatch(`/api/v1/accounts/${a.id}`, { remark: v.length === 0 ? null : v });
        if (autoDelivery !== a.autoDelivery || autoConfirm !== a.autoConfirm) {
          await httpPost(`/api/v1/accounts/${a.id}/control`, {
            expected_version: a.controlVersion,
            auto_delivery_enabled: autoDelivery,
            auto_confirm_enabled: autoConfirm,
            acknowledge_monitoring_scope: true,
          });
        }
        handle.close(true);
        blockRef?.reload();
      } catch (e) {
        hint.textContent = `保存失败:${e instanceof Error ? e.message : String(e)}`;
        save.disabled = false;
      }
    });
    cancel.addEventListener("click", () => handle.close(true));
    const avatarBig = a.avatarUrl !== null && a.avatarUrl.length > 0
      ? (() => {
          const img = el("img", { src: a.avatarUrl, alt: "头像" }) as HTMLImageElement;
          img.style.width = "72px"; img.style.height = "72px"; img.style.borderRadius = "50%"; img.style.objectFit = "cover";
          img.addEventListener("error", () => img.remove());
          return img;
        })()
      : el("span", { class: "avatar", style: "width:72px;height:72px;font-size:24px;" }, a.displayName.slice(0, 1) || "?");
    handle.body.append(
      el("div", { style: "display:flex;gap:16px;align-items:center;margin-bottom:16px;" },
        avatarBig,
        el("div", {},
          el("div", { style: "font-size:var(--text-lg);font-weight:700;" }, a.displayName || "未命名账号"),
          el("div", { class: "muted mono", style: "font-size:var(--text-xs);" }, `ID: ${a.platformUserId}`))),
      el("div", { class: "field" }, el("label", {}, "备注"), remarkInput),
      el("div", { class: "field" }, el("label", {}, "自动化开关"),
        mkSwitch("自动交付", a.autoDelivery, (v) => { autoDelivery = v; }),
        mkSwitch("自动平台确认", a.autoConfirm, (v) => { autoConfirm = v; }),
        el("div", { class: "hint" }, "开关保存时生效;首次启用自动交付需确认监控范围")),
      hint,
      el("div", { style: "display:flex;gap:12px;justify-content:flex-end;margin-top:16px;" }, cancel, save)
    );
  };

  /** 004:删除确认(对齐 Ydisks AccountDeleteDialog)。 */
  const openDeleteDialog = (a: AccountCard): void => {
    const handle = openModal(null, { title: "删除这个账号?" });
    const confirmBtn = btn("确认删除", { variant: "danger" });
    const cancelBtn = btn("取消", { variant: "secondary" });
    const errLine = el("div", { class: "field-error" }, "");
    confirmBtn.addEventListener("click", async () => {
      confirmBtn.disabled = true;
      confirmBtn.textContent = "删除中…";
      cancelBtn.disabled = true;
      try {
        await httpRequest<void>(`/api/v1/accounts/${a.id}`, { method: "DELETE" });
        handle.close(true);
        blockRef?.reload();
      } catch (e) {
        errLine.textContent = `删除失败:${e instanceof Error ? e.message : String(e)}`;
        confirmBtn.disabled = false;
        confirmBtn.textContent = "确认删除";
        cancelBtn.disabled = false;
      }
    });
    cancelBtn.addEventListener("click", () => handle.close(true));
    const avatarS = a.avatarUrl !== null && a.avatarUrl.length > 0
      ? (() => {
          const img = el("img", { src: a.avatarUrl, alt: "" }) as HTMLImageElement;
          img.style.width = "48px"; img.style.height = "48px"; img.style.borderRadius = "50%"; img.style.objectFit = "cover";
          img.addEventListener("error", () => img.remove());
          return img;
        })()
      : el("span", { class: "avatar" }, a.displayName.slice(0, 1) || "?");
    handle.body.append(
      el("div", { class: "confirm-danger-head" },
        el("span", { class: "confirm-danger-ico" }, "⚠"),
        el("div", {},
          el("p", { class: "error-text", style: "margin:0;font-weight:600;" }, "此操作无法撤销"),
          el("p", { class: "muted", style: "margin:0;" }, "删除后加密凭证将被清除、值守停止;订单与交付历史保留(审计可查)。"))),
      el("div", { class: "confirm-danger-target", style: "display:flex;gap:12px;align-items:center;" },
        avatarS,
        el("div", {},
          el("strong", {}, a.displayName || "未命名账号"),
          el("div", { class: "muted mono", style: "font-size:var(--text-xs);" }, `ID: ${a.platformUserId}`))),
      errLine,
      el("div", { style: "display:flex;gap:12px;justify-content:flex-end;margin-top:16px;" }, cancelBtn, confirmBtn)
    );
  };
  /** 006:卡片形态(与横条共享装配,仅布局差异;保留 ID 一键复制)。 */
  const renderCard = (a: AccountCard): HTMLElement => {
    const { box: badgeBox, sync: syncBadge } = badgePairEl(a);
    const verifyBadge = el("span", { class: "badge badge-neutral" }, "");
    const syncVerify = attachVerifyBadge(a, verifyBadge);

    const copyBtn = el("button", { class: "btn btn-ghost btn-sm", type: "button", title: "复制 ID" }, "复制") as HTMLButtonElement;
    copyBtn.addEventListener("click", () => {
      void navigator.clipboard.writeText(a.platformUserId).then(
        () => {
          copyBtn.textContent = "已复制";
        },
        () => {
          copyBtn.textContent = "复制失败";
        }
      );
    });

    const main = entryMainEl(a, badgeBox, verifyBadge, [copyBtn]);
    return el(
      "div",
      { class: "account-card" },
      el("div", { class: "account-head" }, avatarEl(a), main),
      actionsEl(a, syncBadge, syncVerify)
    );
  };



  /** 启停执行(US3-4):确认后本地进入"暂停中"过渡态,轮询单账号至状态落定。 */
  const applyControl = (a: AccountCard, enabling: boolean, syncBadge: () => void): Promise<void> => {
    a.runEnabled = enabling;
    pausingIds.add(a.id);
    syncBadge();
    return (async () => {
      await httpPost(`/api/v1/accounts/${a.id}/control`, {
        expected_version: a.controlVersion,
        run_enabled: enabling,
        acknowledge_monitoring_scope: true
      });
      const deadline = Date.now() + 30_000;
      await new Promise<void>((resolve) => {
        const timer = window.setInterval(() => {
          void (async () => {
            try {
              const fresh = parseAccountCard(await httpGet(`/api/v1/accounts/${a.id}`));
              a.connectionState = fresh.connectionState;
              a.controlVersion = fresh.controlVersion;
              a.monitoringSince = fresh.monitoringSince;
              syncBadge();
              const settled = enabling
                ? fresh.connectionState !== "paused"
                : fresh.connectionState === "paused";
              if (settled || Date.now() > deadline) {
                window.clearInterval(timer);
                resolve();
              }
            } catch {
              if (Date.now() > deadline) {
                window.clearInterval(timer);
                resolve();
              }
            }
          })();
        }, 2000);
      });
    })()
      .catch(() => {
        a.runEnabled = !enabling; // 失败回滚视觉状态
        syncBadge();
      })
      .finally(() => {
        pausingIds.delete(a.id);
        syncBadge();
      });
  };

  const emptyState = (): HTMLElement =>
    el(
      "div",
      { class: "empty-state" },
      el("div", { class: "empty-ico" }, "👤"),
      el("div", {}, "还没有接入闲鱼账号"),
      el("div", { class: "muted" }, "扫码接入后即可配置商品与自动发货")
    );

  // 页头(006 FR-001)+ 工具栏(FR-002:搜索居左,主按钮居右)+ 计数横幅
  const head = el(
    "div",
    { class: "page-head" },
    el("h1", { class: "page-head-title" }, "账号管理"),
    el("p", { class: "page-head-sub" }, "管理您的闲鱼授权账号及设置。建议给账号填写备注，便于多账号区分。")
  );
  const searchInput = el("input", {
    class: "search-input",
    type: "search",
    placeholder: "搜索昵称 / 备注 / 账号ID",
    "aria-label": "搜索昵称 / 备注 / 账号ID"
  }) as HTMLInputElement;
  searchInput.addEventListener("input", () => {
    query = searchInput.value;
    rerenderLocal();
  });
  const qrBtn = btn("扫码添加新账号", { variant: "primary" });
  qrBtn.prepend(icon("qr"));
  qrBtn.classList.add("btn-with-icon");
  const toolbarActions = el("div", { class: "toolbar-actions" }, qrBtn);
  const toolbar = el(
    "div",
    { class: "account-toolbar" },
    el("div", { class: "toolbar-search" }, searchInput),
    toolbarActions
  );

  root.append(head, toolbar, banner);
  const listHost = el("div", { class: "account-list-host" });
  root.append(listHost);

  let blockRef: ReturnType<typeof asyncBlock> | null = null;
  const block = asyncBlock(listHost, load, { skeletonLines: 6, empty: emptyState });
  blockRef = block;

  // 006 US3:双图标分段视图切换(仅图标无文字,激活高亮,悬停提示,记忆偏好),
  // 置于"扫码添加新账号"主按钮左侧(FR-002/FR-009)
  const viewToggle = createSegmented({
    options: [
      { value: "row", icon: "list", label: "列表视图" },
      { value: "card", icon: "grid", label: "卡片视图" }
    ],
    value: viewMode,
    onSelect: (v) => {
      viewMode = v;
      writeViewPreference(v);
      if (cache !== null && inflight === 0) rerenderLocal();
      else blockRef?.reload();
    }
  });
  toolbarActions.prepend(viewToggle);
  qrBtn.addEventListener("click", () => {
    openQrModal(qrBtn, {
      onAuthorized: () => {
        stopTicker?.();
        block.reload();
      }
    });
  });

  return () => {
    stopTicker?.();
    block.dispose();
  };
};

/** 设置页(US7/T050/T051,FR-019):运行信息/安全/关于/备份恢复。
 *  契约边界:仅消费 GET /capabilities、GET /auth/session、GET /restore。 */
import { el } from "../../shared/dom";
import { httpGet } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { btn, muted } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { confirmDialog } from "../../ui/modal";
import { formatRelative } from "../../ui/time";
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

export const settingsPage: PageFactory = (root) => {
  const stop = asyncBlock(root, async () => {
    const [capsRaw, restoreRaw] = await Promise.all([
      httpGet("/api/v1/capabilities"),
      httpGet("/api/v1/restore").catch(() => null)
    ]);
    const caps = parseCapabilities(capsRaw);
    const restore = isRecord(restoreRaw) ? restoreRaw : null;
    const page = el("div", { class: "settings-grid" });

    // 运行信息卡(T050)
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

    // 安全卡(T050):会话与密码重置指引
    page.append(
      el(
        "section",
        { class: "card" },
        el("h2", { class: "card-title" }, "安全"),
        el("dl", { class: "kv" },
          el("div", {}, el("dt", {}, "当前管理员"), el("dd", {}, "admin")),
          el("div", {}, el("dt", {}, "会话有效期"), el("dd", {}, "登录后 12 小时(绝对)"))
        ),
        muted("忘记密码:停止服务后运行 qing-delivery.exe init-admin --reset(需本机同用户);重置会使所有会话失效并暂停全部账号。")
      )
    );

    // 备份/恢复卡(T051)
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

    // 关于卡(T050):本地运行边界
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

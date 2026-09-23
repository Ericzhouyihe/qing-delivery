import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { nextQrStage } from "./qr-modal";
import { accountsPage, parseAccountCard } from "./page";
import { el } from "../../shared/dom";

describe("扫码阶段映射(US3/T032)", () => {
  it("引擎状态 → 界面阶段", () => {
    expect(nextQrStage("awaiting_scan")).toBe("showing");
    expect(nextQrStage("awaiting_authorization")).toBe("showing");
    expect(nextQrStage("verification_required")).toBe("showing");
    expect(nextQrStage("authorized")).toBe("success");
    expect(nextQrStage("expired")).toBe("expired");
    expect(nextQrStage("cancelled")).toBe("failed");
    expect(nextQrStage("failed")).toBe("failed");
    expect(nextQrStage("brand_new")).toBe("failed"); // 未知状态兜底,不崩溃
  });
});

describe("账号解析", () => {
  it("解析 control 嵌套与缺省", () => {
    const a = parseAccountCard({
      id: "acct-1",
      platform_user_id: "1234567890abcdef",
      display_name: "小店",
      connection_state: "online",
      control: { run_enabled: true, auto_delivery_enabled: false, auto_confirm_enabled: false, transition: "stable" },
      control_version: 3,
      monitoring_since: "2026-09-22T02:00:00Z"
    });
    expect(a.runEnabled).toBe(true);
    expect(a.autoDelivery).toBe(false);
    expect(a.controlVersion).toBe(3);
    expect(a.monitoringSince).not.toBeNull();

    const b = parseAccountCard({ id: "x" });
    expect(b.connectionState).toBe("paused");
    expect(b.controlVersion).toBe(1);
  });
});

describe("启停确认流(US3-4/F 约定:取消不发起任何变更)", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn().mockImplementation((input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/api/v1/accounts") && !url.includes("/control")) {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              items: [
                {
                  id: "acct-1",
                  platform_user_id: "1234567890abcdef",
                  display_name: "小店",
                  connection_state: "online",
                  control: { run_enabled: true, auto_delivery_enabled: true, auto_confirm_enabled: false },
                  control_version: 2,
                  monitoring_since: null
                }
              ]
            }),
            { status: 200, headers: { "Content-Type": "application/json" } }
          )
        );
      }
      return Promise.resolve(new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } }));
    });
    vi.stubGlobal("fetch", fetchMock);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    document.querySelectorAll(".overlay").forEach((n) => n.remove());
  });

  it("切换开关弹出确认;取消后无 control 请求、开关回弹", async () => {
    const root = el("div");
    document.body.append(root);
    const dispose = accountsPage(root);
    await vi.waitFor(() => {
      expect(root.querySelector(".account-card")).not.toBeNull();
    });

    const toggle = root.querySelector<HTMLInputElement>("input[type=checkbox]")!;
    expect(toggle.checked).toBe(true);
    toggle.checked = false; // 模拟用户点击停用
    toggle.dispatchEvent(new Event("change"));

    await vi.waitFor(() => {
      expect(document.querySelector(".overlay .modal-title")?.textContent).toContain("停用账号");
    });
    const cancel = Array.from(document.querySelectorAll(".overlay button")).find(
      (b) => b.textContent === "取消"
    ) as HTMLButtonElement;
    cancel.click();
    await Promise.resolve();

    const controlCalls = fetchMock.mock.calls.filter((c) => String(c[0]).includes("/control"));
    expect(controlCalls.length).toBe(0);
    expect(toggle.checked).toBe(true);

    expect(typeof dispose).toBe("function");
    (dispose as () => void)();
  });
});

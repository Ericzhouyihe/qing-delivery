/** US8/T055:密码长度/一致性校验、错误行内呈现、过期回落与返回路由。 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { el } from "../shared/dom";
import { validateInit, renderAuth } from "./auth";
import { session } from "./session";

describe("validateInit 初始化校验(US8-1)", () => {
  it("长度不足 12 拒绝并给出当前长度", () => {
    const r = validateInit("short", "short");
    expect(r.ok).toBe(false);
    expect(r.error).toContain("12");
  });

  it("两次不一致拒绝", () => {
    const r = validateInit("a-very-long-pass-1", "a-very-long-pass-2");
    expect(r.ok).toBe(false);
    expect(r.error).toContain("不一致");
  });

  it("合法输入通过", () => {
    expect(validateInit("a-very-long-pass-1", "a-very-long-pass-1").ok).toBe(true);
  });
});

describe("renderAuth(US8 行内错误与登录中状态)", () => {
  afterEach(() => {
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("未初始化:渲染初始化卡片;不一致时行内错误,不发起请求", async () => {
    session.initialized = false;
    session.authenticated = false;
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const root = el("div", {});
    renderAuth(root, () => undefined);
    const inputs = root.querySelectorAll<HTMLInputElement>("input[type=password]");
    expect(inputs.length).toBe(2);
    const button = root.querySelector("button") as HTMLButtonElement;
    inputs[0]!.value = "a-very-long-pass-1";
    inputs[1]!.value = "different-long-2";
    button.click();
    await vi.waitFor(() => {
      expect(root.querySelector(".error-inline")?.textContent).toContain("不一致");
    });
    expect(fetchMock).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("已初始化:登录失败行内呈现服务端错误(密码错误不弹窗)", async () => {
    session.initialized = true;
    session.authenticated = false;
    // 每次调用返回全新 Response:body 只能读取一次,组件会对 401 做一次令牌刷新重试
    const fetchMock = vi.fn().mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify({ error: { code: "invalid_credentials", message: "密码不正确" } }), {
          status: 401,
          headers: { "Content-Type": "application/json" }
        })
      )
    );
    vi.stubGlobal("fetch", fetchMock);
    const root = el("div", {});
    let authed = 0;
    renderAuth(root, () => {
      authed += 1;
    });
    const input = root.querySelector<HTMLInputElement>("input[type=password]");
    const button = root.querySelector("button") as HTMLButtonElement;
    input!.value = "whatever-password";
    await button.click();
    await vi.waitFor(() => {
      expect(root.querySelector(".error-inline")?.textContent).toContain("密码不正确");
    });
    expect(authed).toBe(0);
    // 登录中按钮态已恢复
    expect(button.disabled).toBe(false);
    vi.unstubAllGlobals();
  });

  it("登录成功:消费 returnTo 并返回原页面(而非固定概览)", async () => {
    session.initialized = true;
    session.authenticated = false;
    history.pushState(null, "", "/orders");
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({ initialized: true, authenticated: true, csrf_token: "t", administrator: { id: "a", display_name: "管理员" } }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      )
    );
    vi.stubGlobal("fetch", fetchMock);
    const root = el("div", {});
    let authed = 0;
    renderAuth(root, () => {
      authed += 1;
    });
    const input = root.querySelector<HTMLInputElement>("input[type=password]");
    const button = root.querySelector("button") as HTMLButtonElement;
    input!.value = "a-very-long-pass-1";
    await button.click();
    await vi.waitFor(() => expect(authed).toBe(1));
    // 当前仍在 /orders(URL 未被改写为 /)
    expect(location.pathname).toBe("/orders");
    vi.unstubAllGlobals();
    history.pushState(null, "", "/");
  });
});

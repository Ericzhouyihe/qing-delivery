import "./styles/index.css";
import { installRouter } from "./app/router";
import { bootstrapSession } from "./app/session";
import { initTheme } from "./ui/theme";

// 渲染前应用主题,避免明暗闪烁(FR-002)
initTheme();

async function start(): Promise<void> {
  const root = document.getElementById("app");
  if (root === null) throw new Error("缺少 #app 挂载点");
  await bootstrapSession();
  installRouter(root);
}

void start();

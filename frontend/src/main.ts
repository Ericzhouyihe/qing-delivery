import "./shared/styles.css";
import { installRouter } from "./app/router";
import { bootstrapSession } from "./app/session";

async function start(): Promise<void> {
  const root = document.getElementById("app");
  if (root === null) throw new Error("缺少 #app 挂载点");
  await bootstrapSession();
  installRouter(root);
}

void start();

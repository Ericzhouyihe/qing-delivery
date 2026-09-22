import { defineConfig } from "vitest/config";

// 开发代理维持同源语义：页面请求经 Vite 代理到本地后端，
// 不设置任意来源 CORS；后端仍按契约校验 Host/Origin/CSRF。
export default defineConfig({
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": {
        target: "http://127.0.0.1:59189",
        changeOrigin: false
      }
    }
  },
  build: {
    outDir: "../src/webui",
    emptyOutDir: true
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"]
  }
});

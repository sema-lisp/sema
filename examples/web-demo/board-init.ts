import { SemaWeb } from "../../packages/sema-web/src/index.ts";

try {
  const web = await SemaWeb.create({
    llmProxy: "http://localhost:3002",
  });
  (window as any).__semaWeb = web;
} catch (e: any) {
  console.error("SemaWeb init failed:", e);
  (window as any).__semaInitError = String(e);
  const app = document.getElementById("app");
  if (app) app.textContent = "Init failed: " + e.message;
}

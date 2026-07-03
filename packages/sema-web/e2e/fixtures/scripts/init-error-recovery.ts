import { SemaWeb } from "../../../src/index.ts";
try {
  const web = await SemaWeb.init();
  (window as any).__semaWeb = web;
} catch (e) {
  console.error("SemaWeb init failed:", e);
  (window as any).__semaInitError = String(e);
} finally {
  (window as any).__semaInitialized = true;
}

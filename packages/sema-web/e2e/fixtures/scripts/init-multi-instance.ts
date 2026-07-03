import { SemaWeb } from "../../../src/index.ts";
try {
  const webA = await SemaWeb.create({ autoLoad: false });
  const webB = await SemaWeb.create({ autoLoad: false });
  (window as any).__semaWebA = webA;
  (window as any).__semaWebB = webB;
} catch (e) {
  console.error("SemaWeb init failed:", e);
  (window as any).__semaInitError = String(e);
} finally {
  (window as any).__semaInitialized = true;
}

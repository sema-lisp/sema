import { SEMA_IDENT_RE } from "./handles.js";

export interface SemaCallback {
  (...args: any[]): any;
  __semaCallbackHandle?: number;
  __semaRelease?: () => void;
}

interface GlobalInvoker {
  invokeGlobal(name: string, ...args: any[]): any;
}

export function toInvokableCallback(
  value: unknown,
  interp: GlobalInvoker,
  label: string,
): SemaCallback {
  if (typeof value === "function") {
    return value as SemaCallback;
  }

  if (typeof value === "string" && SEMA_IDENT_RE.test(value)) {
    return ((...args: any[]) => interp.invokeGlobal(value, ...args)) as SemaCallback;
  }

  throw new Error(`Invalid ${label}: expected function value or callback name`);
}

export function releaseCallback(value: unknown): void {
  if (typeof value === "function") {
    (value as SemaCallback).__semaRelease?.();
  }
}

import { beforeEach } from "vitest";

class MemoryStorage implements Storage {
  #data = new Map<string, string>();

  get length(): number {
    return this.#data.size;
  }

  clear(): void {
    this.#data.clear();
  }

  getItem(key: string): string | null {
    return this.#data.has(key) ? this.#data.get(key)! : null;
  }

  key(index: number): string | null {
    return Array.from(this.#data.keys())[index] ?? null;
  }

  removeItem(key: string): void {
    this.#data.delete(key);
  }

  setItem(key: string, value: string): void {
    this.#data.set(String(key), String(value));
  }
}

const local = new MemoryStorage();
const session = new MemoryStorage();

Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  enumerable: true,
  value: local,
  writable: true,
});

Object.defineProperty(globalThis, "sessionStorage", {
  configurable: true,
  enumerable: true,
  value: session,
  writable: true,
});

if (typeof window !== "undefined") {
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    enumerable: true,
    value: local,
    writable: true,
  });

  Object.defineProperty(window, "sessionStorage", {
    configurable: true,
    enumerable: true,
    value: session,
    writable: true,
  });
}

beforeEach(() => {
  local.clear();
  session.clear();
});

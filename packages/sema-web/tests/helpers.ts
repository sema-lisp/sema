export interface MockEvalResult {
  value: string | null;
  output: string[];
  error: string | null;
}

export function createMockInterpreter() {
  const functions = new Map<string, (...args: any[]) => any>();
  const evalCalls: string[] = [];

  return {
    registerFunction(name: string, fn: (...args: any[]) => any) {
      functions.set(name, fn);
    },
    invokeGlobal(name: string, ...args: any[]) {
      evalCalls.push(`(${name} ${args.map((arg) => JSON.stringify(arg)).join(" ")})`);
      const fn = functions.get(name);
      return fn ? fn(...args) : null;
    },
    evalStr(code: string): MockEvalResult {
      evalCalls.push(code);
      // Try to execute simple function calls for macro registration
      const match = code.match(/^\((\S+)\s*(.*)\)$/);
      if (match) {
        const fn = functions.get(match[1]);
        if (fn) {
          try { fn(); } catch {}
        }
      }
      return { value: null, output: [], error: null };
    },
    evalStrAsync(code: string): Promise<MockEvalResult> {
      return Promise.resolve(this.evalStr(code));
    },
    getFunction(name: string) { return functions.get(name); },
    getEvalCalls() { return evalCalls; },
    functions,
  };
}

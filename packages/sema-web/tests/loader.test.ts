import { afterEach, describe, expect, it, vi } from "vitest";
import { loadScripts } from "../src/loader.js";

describe("loadScripts", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("loads external .sema scripts as source", async () => {
    document.body.innerHTML = '<script type="text/sema" src="/app.sema"></script>';

    const evalStrAsync = vi.fn().mockResolvedValue({
      value: "ok",
      output: [],
      error: null,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        text: () => Promise.resolve('(println "hello")'),
      }),
    );

    const results = await loadScripts({
      evalStr: vi.fn(),
      evalStrAsync,
    });

    expect(evalStrAsync).toHaveBeenCalledWith('(println "hello")');
    expect(results).toHaveLength(1);
    expect(results[0]?.error).toBeNull();
  });

  it("loads external .vfs scripts as compiled archives", async () => {
    document.body.innerHTML = '<script type="text/sema" src="/app.vfs"></script>';

    const archiveBytes = new Uint8Array([0, 1, 2, 3]);
    const loadArchive = vi.fn().mockReturnValue({
      ok: true,
      entryPoint: "__main__.semac",
      fileCount: 2,
      semaVersion: "1.9.0",
      buildTarget: "web",
      buildTimestamp: "0",
      error: null,
    });
    const runEntryAsync = vi.fn().mockResolvedValue({
      value: "done",
      output: [],
      error: null,
    });

    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        arrayBuffer: () =>
          Promise.resolve(
            archiveBytes.buffer.slice(
              archiveBytes.byteOffset,
              archiveBytes.byteOffset + archiveBytes.byteLength,
            ),
          ),
      }),
    );

    const results = await loadScripts({
      evalStr: vi.fn(),
      loadArchive,
      runEntryAsync,
    });

    expect(loadArchive).toHaveBeenCalledTimes(1);
    expect(loadArchive.mock.calls[0]?.[0]).toBeInstanceOf(Uint8Array);
    expect(runEntryAsync).toHaveBeenCalledWith("__main__.semac");
    expect(results).toHaveLength(1);
    expect(results[0]?.value).toBe("done");
  });

  it("surfaces archive compatibility failures", async () => {
    document.body.innerHTML = '<script type="text/sema" src="/app.vfs"></script>';

    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        arrayBuffer: () => Promise.resolve(new Uint8Array([1, 2, 3]).buffer),
      }),
    );

    const results = await loadScripts({
      evalStr: vi.fn(),
      loadArchive: vi.fn(() => {
        throw new Error("archive version mismatch: built with Sema 0.0.0, runtime is 1.9.0");
      }),
      runEntryAsync: vi.fn(),
    });

    expect(results[0]?.error).toContain("archive version mismatch");
  });

  it("uses the requested script MIME type and treats empty scripts as successful no-ops", async () => {
    document.body.innerHTML = `
      <script type="text/sema">(println "ignored")</script>
      <script type="application/sema">   </script>
    `;

    const evalStr = vi.fn();
    const results = await loadScripts({ evalStr }, { type: "application/sema" });

    expect(evalStr).not.toHaveBeenCalled();
    expect(results).toEqual([{ value: null, output: [], error: null }]);
  });

  it("falls back to synchronous evalStr and logs output lines", async () => {
    document.body.innerHTML = '<script type="text/sema">(println "sync")</script>';
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    const evalStr = vi.fn().mockReturnValue({
      value: "ok",
      output: ["one", "two"],
      error: null,
    });

    const results = await loadScripts({ evalStr });

    expect(evalStr).toHaveBeenCalledWith('(println "sync")');
    expect(logSpy).toHaveBeenCalledWith("[sema] one");
    expect(logSpy).toHaveBeenCalledWith("[sema] two");
    expect(results[0]?.value).toBe("ok");
  });

  it("continues after a script evaluation throws", async () => {
    document.body.innerHTML = `
      <script type="text/sema">bad</script>
      <script type="text/sema">good</script>
    `;
    vi.spyOn(console, "error").mockImplementation(() => {});
    const evalStr = vi.fn()
      .mockImplementationOnce(() => {
        throw new Error("boom");
      })
      .mockReturnValueOnce({ value: "ok", output: [], error: null });

    const results = await loadScripts({ evalStr });

    expect(results).toHaveLength(2);
    expect(results[0]?.error).toBe("Evaluation error: boom");
    expect(results[1]?.value).toBe("ok");
  });

  it("records interpreter-reported source errors without stopping later scripts", async () => {
    document.body.innerHTML = `
      <script type="text/sema">bad</script>
      <script type="text/sema">good</script>
    `;
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const evalStrAsync = vi.fn()
      .mockResolvedValueOnce({ value: null, output: [], error: "syntax bad" })
      .mockResolvedValueOnce({ value: "ok", output: [], error: null });

    const results = await loadScripts({ evalStr: vi.fn(), evalStrAsync });

    expect(errorSpy).toHaveBeenCalledWith("[sema-web] Error in inline script: syntax bad");
    expect(results[0]?.error).toBe("syntax bad");
    expect(results[1]?.value).toBe("ok");
  });

  it("surfaces non-OK external source fetches and network failures", async () => {
    document.body.innerHTML = `
      <script type="text/sema" src="/missing.sema"></script>
      <script type="text/sema" src="/offline.sema"></script>
    `;
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal(
      "fetch",
      vi.fn()
        .mockResolvedValueOnce({ ok: false, status: 404, statusText: "Not Found" })
        .mockRejectedValueOnce(new Error("network down")),
    );

    const results = await loadScripts({ evalStr: vi.fn() });

    expect(results[0]?.error).toBe("Failed to fetch /missing.sema: 404 Not Found");
    expect(results[1]?.error).toBe("Failed to fetch /offline.sema: network down");
  });

  it("requires archive support before loading .vfs scripts, including query-string URLs", async () => {
    document.body.innerHTML = '<script type="text/sema" src="/app.vfs?cache=1"></script>';
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({
      ok: true,
      arrayBuffer: () => Promise.resolve(new Uint8Array([1, 2, 3]).buffer),
    }));

    const results = await loadScripts({ evalStr: vi.fn() });

    expect(results[0]?.error).toBe("Runtime does not support compiled web archives: /app.vfs?cache=1");
  });

  it("surfaces archive load results that are not ok or lack an entry point", async () => {
    document.body.innerHTML = `
      <script type="text/sema" src="/bad.vfs"></script>
      <script type="text/sema" src="/empty.vfs"></script>
    `;
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({
      ok: true,
      arrayBuffer: () => Promise.resolve(new Uint8Array([1, 2, 3]).buffer),
    }));
    const loadArchive = vi.fn()
      .mockReturnValueOnce({
        ok: false,
        entryPoint: null,
        fileCount: 0,
        semaVersion: null,
        buildTarget: null,
        buildTimestamp: null,
        error: "bad archive",
      })
      .mockReturnValueOnce({
        ok: true,
        entryPoint: null,
        fileCount: 1,
        semaVersion: "1.9.0",
        buildTarget: "web",
        buildTimestamp: "0",
        error: null,
      });

    const results = await loadScripts({ evalStr: vi.fn(), loadArchive, runEntry: vi.fn() });

    expect(results[0]?.error).toBe("bad archive");
    expect(results[1]?.error).toBe("Archive /empty.vfs did not provide an entry point");
  });

  it("supports synchronous archive entry execution and logs archive output/errors", async () => {
    document.body.innerHTML = '<script type="text/sema" src="/app.vfs"></script>';
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({
      ok: true,
      arrayBuffer: () => Promise.resolve(new Uint8Array([1, 2, 3]).buffer),
    }));

    const results = await loadScripts({
      evalStr: vi.fn(),
      loadArchive: vi.fn().mockReturnValue({
        ok: true,
        entryPoint: "main.semac",
        fileCount: 1,
        semaVersion: "1.9.0",
        buildTarget: "web",
        buildTimestamp: "0",
        error: null,
      }),
      runEntry: vi.fn().mockReturnValue({
        value: null,
        output: ["archive line"],
        error: "runtime err",
      }),
    });

    expect(logSpy).toHaveBeenCalledWith("[sema] archive line");
    expect(errorSpy).toHaveBeenCalledWith("[sema-web] Error in /app.vfs: runtime err");
    expect(results[0]?.error).toBe("runtime err");
  });
});

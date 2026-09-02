/**
 * LLM proxy bindings for Sema — registers `llm/*` namespace functions.
 *
 * Since LLM API keys cannot be safely stored in the browser, this module
 * bridges the Sema `llm/*` API to a backend proxy server via `http/post`.
 * The proxy holds API keys and forwards requests to the actual LLM providers.
 *
 * ## Architecture
 *
 * ```
 * Browser (Sema code)          Backend proxy server
 * ┌──────────────────┐         ┌──────────────────┐
 * │ (llm/chat ...)   │──POST──▶│ /api/llm/chat    │──▶ OpenAI / Anthropic / etc.
 * │ (llm/complete ..)│──POST──▶│ /api/llm/complete │──▶ ...
 * │ (llm/embed ...)  │──POST──▶│ /api/llm/embed    │──▶ ...
 * │ (llm/chat-stream)│──POST──▶│ /api/llm/stream   │──▶ SSE tokens ──▶ signal
 * └──────────────────┘         └──────────────────┘
 * ```
 *
 * ## Implementation
 *
 * Most LLM functions are defined as pure Sema code using `http/post`,
 * `json/encode`, and `json/decode`. This piggybacks on the WASM
 * interpreter's HTTP replay mechanism: when called via `evalAsync()`,
 * `http/post` calls are intercepted, executed via browser `fetch()`,
 * and the results are replayed back transparently.
 *
 * `llm/chat-stream` is different — it is a JS-registered function that
 * returns a reactive signal ID. The signal value is `{text, done, error}`
 * and updates progressively as SSE tokens arrive from the proxy. Components
 * that `(deref stream)` the signal auto-re-render via the reactive system.
 *
 * ## Usage
 *
 * ```js
 * const web = await SemaWeb.create({
 *   llmProxy: "https://my-backend.example.com/api/llm",
 * });
 * // Must use evalAsync for HTTP-based LLM calls:
 * await web.evalAsync('(llm/chat (list (message :user "Hi")) {:model "gpt-4o"})');
 *
 * // Streaming (returns a signal, works synchronously):
 * web.eval('(def s (llm/chat-stream (list (message :user "Hi")) {:model "gpt-4o"}))');
 * // (deref s) → {:text "Hello wo" :done false}
 * // ... later: (deref s) → {:text "Hello world!" :done true}
 * ```
 *
 * @module
 */

import { signal } from "@preact/signals-core";
import type { SemaWebContext } from "./context.js";
import { getCurrentOwnerId, registerStream, unregisterStream } from "./context.js";
import { openSseStream } from "./sse.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
  evalStr(code: string): { value: string | null; output: string[]; error: string | null };
}

/** Configuration for the LLM proxy. */
export interface LlmProxyOptions {
  /**
   * Base URL of the LLM proxy server.
   * Endpoints are appended as: `{url}/complete`, `{url}/chat`, etc.
   */
  url: string;

  /**
   * Optional authorization header value.
   * Sent as `Authorization: Bearer {token}` on every request.
   * This is for authenticating the browser client to the proxy —
   * NOT the LLM API key (which should be stored server-side).
   */
  token?: string;

  /**
   * Optional custom headers to include in every proxy request.
   */
  headers?: Record<string, string>;

  /**
   * Reserved. Proxy requests run through the interpreter's `http/post`, which
   * has no per-request timeout in the browser, so this value is not applied.
   */
  timeout?: number;
}

/** The reactive value a `llm/chat-stream` signal holds. */
export interface LlmStreamState {
  text: string;
  done: boolean;
  error: string | null;
}

/**
 * Build the header set sent on every proxy request.
 *
 * Precedence, from weakest to strongest:
 * 1. `Content-Type: application/json` — the proxy contract, but overridable
 *    (a proxy may want an explicit charset).
 * 2. `opts.headers` — caller-supplied.
 * 3. `Authorization: Bearer <token>` — **wins** whenever a token is configured.
 *
 * Auth deliberately sits on top. A caller who sets both a `token` and their own
 * `Authorization` header has expressed a contradiction, and the safe reading is
 * that the explicitly-named credential is the intended one — silently
 * downgrading it to whatever happened to be in a generic header bag is how
 * requests end up unauthenticated in production. With no token configured a
 * custom `Authorization` header passes through untouched, so proxies using a
 * non-Bearer scheme still work.
 *
 * Exported for testing and for callers that need to reproduce the exact header
 * set (a service worker, a custom fetch wrapper).
 */
export function buildProxyHeaders(opts: LlmProxyOptions): Record<string, string> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };

  for (const [key, value] of Object.entries(opts.headers ?? {})) {
    headers[key] = value;
  }

  if (opts.token) {
    const overridden = Object.keys(opts.headers ?? {}).find(
      (k) => k.toLowerCase() === "authorization",
    );
    if (overridden) {
      console.warn(
        `[sema-web] llmProxy: both \`token\` and a custom \`${overridden}\` header were ` +
          "provided. The token wins. Drop one to silence this warning.",
      );
      delete headers[overridden];
    }
    headers["Authorization"] = `Bearer ${opts.token}`;
  }

  return headers;
}

/**
 * Serialize the proxy headers as a Sema map literal.
 *
 * Every key and value goes through {@link escapeSemaString}: these strings are
 * interpolated into Sema *source*, so an unescaped quote in a token or a custom
 * header would close the string literal and let the remainder be parsed as
 * code. Exported so tests exercise this exact serializer rather than a
 * re-implementation that would quietly diverge from it.
 */
export function buildProxyHeadersMap(opts: LlmProxyOptions): string {
  const headers = buildProxyHeaders(opts);
  return `{${Object.entries(headers)
    .map(([k, v]) => `"${escapeSemaString(k)}" "${escapeSemaString(v)}"`)
    .join(" ")}}`;
}

/**
 * Accumulate the proxy's streaming protocol into a signal value.
 *
 * The protocol is newline-delimited JSON over SSE:
 * `{"type":"token","text":"..."}`, `{"type":"done"}`, `{"type":"error","error":"..."}`.
 * The OpenAI-style `[DONE]` sentinel is also accepted, because proxies that
 * pass a provider stream straight through emit it and `JSON.parse` would
 * otherwise report it as a corrupt payload.
 *
 * **Terminal states latch.** Once the stream reports done or error, later
 * events are ignored. Without this a trailing token after an error would reset
 * `error` to `null` and `done` to `false` — the UI would clear the error
 * message it just showed and hang on a stream that already finished.
 *
 * Exported for testing: this is the whole protocol contract, and it is worth
 * being able to exercise without a network.
 */
export function createStreamAccumulator(emit: (state: LlmStreamState) => void) {
  let text = "";
  let error: string | null = null;
  let terminal = false;

  const state = (): LlmStreamState => ({ text, done: terminal, error });

  return {
    get state(): LlmStreamState {
      return state();
    },

    /** Handle one SSE `data:` payload. */
    event(data: string): void {
      if (terminal || !data) return;

      // Provider passthrough sentinel — a valid end-of-stream marker, not JSON.
      if (data.trim() === "[DONE]") {
        terminal = true;
        emit(state());
        return;
      }

      let parsed: any;
      try {
        parsed = JSON.parse(data);
      } catch {
        terminal = true;
        error = "Invalid stream payload";
        emit(state());
        return;
      }

      if (parsed?.type === "token" && typeof parsed.text === "string") {
        text += parsed.text;
        emit(state());
        return;
      }
      if (parsed?.type === "done") {
        terminal = true;
        emit(state());
        return;
      }
      if (parsed?.type === "error") {
        terminal = true;
        error = typeof parsed.error === "string" ? parsed.error : "Stream error";
        emit(state());
        return;
      }
      // Unknown frame types are ignored rather than treated as corruption:
      // proxies add metadata frames (usage, model echo) and an app should not
      // break because its proxy got more informative.
    },

    /** Handle a transport-level failure. */
    fail(message: string): void {
      if (terminal) return;
      terminal = true;
      error = message;
      emit(state());
    },

    /**
     * Handle the stream closing.
     *
     * A close with no prior terminal frame means the connection ended without
     * a `done` — mark it finished so consumers stop waiting, but do not invent
     * an error: a proxy that closes cleanly after its last token is behaving
     * acceptably, just tersely.
     */
    close(): void {
      if (terminal) return;
      terminal = true;
      emit(state());
    },
  };
}

/**
 * Register all `llm/*` namespace functions that proxy to a backend server.
 *
 * Functions are defined as Sema code using `http/post` and `json/encode`/
 * `json/decode` — this ensures they work naturally with the WASM async
 * eval loop. When called via `evalAsync()`, HTTP requests are intercepted
 * by the WASM replay mechanism and executed via browser `fetch()`.
 *
 * Functions registered:
 * - `llm/complete` — simple text completion
 * - `llm/chat` — chat with messages list
 * - `llm/send` — send a prompt/messages object
 * - `llm/extract` — structured data extraction
 * - `llm/classify` — classification
 * - `llm/embed` — text embeddings
 * - `llm/list-models` — list available models from the proxy
 * - `llm/proxy-url` — return the configured proxy URL
 * - `message` — helper to build chat messages
 */
export function registerLlmBindings(
  interp: SemaInterpreterLike,
  opts: LlmProxyOptions,
  ctx: SemaWebContext,
): void {
  const proxyUrl = opts.url.replace(/\/+$/, "");

  // One header set for both transports. These used to be built twice — a Sema
  // map literal for the `http/post` path and a JS object for the streaming
  // path — with *opposite* precedence: custom headers overrode the token in
  // one and lost to it in the other. Identical config therefore authenticated
  // differently depending on whether the call streamed. Building once removes
  // the class of bug, and dedupes keys for free (the literal could previously
  // emit `"Authorization"` twice).
  const proxyHeaders = buildProxyHeaders(opts);
  const headersMap = buildProxyHeadersMap(opts);

  // Register a simple JS function for the proxy URL (sync, no HTTP needed)
  interp.registerFunction("llm/proxy-url", () => proxyUrl);

  // --- llm/chat-stream: streaming chat that returns a reactive signal ---
  //
  // Returns a signal ID. The signal value is {text: "", done: false, error: null}.
  // As SSE tokens arrive from the proxy, `text` accumulates and components
  // that `(deref stream)` auto-re-render. When the stream finishes,
  // `done` becomes true.
  //
  // Usage from Sema:
  //   (def s (llm/chat-stream messages))        ; with defaults
  //   (def s (llm/chat-stream messages opts))    ; with options map
  //   (deref s) ;; → {:text "..." :done false :error nil}
  // __llm/chat-stream-raw takes JSON strings (serialized on the Sema side)
  interp.registerFunction("__llm/chat-stream-raw", (messagesJson: string, optsJson?: string) => {
    const messages = JSON.parse(messagesJson);
    const streamOpts = optsJson ? JSON.parse(optsJson) : {};

    const id = ctx.nextSignalId++;
    const s = signal<LlmStreamState>({
      text: "",
      done: false,
      error: null,
    });
    ctx.signals.set(id, s as any);

    // Strip colon prefixes from Sema keyword keys (":role" → "role")
    function stripColonKeys(obj: any): any {
      if (Array.isArray(obj)) return obj.map(stripColonKeys);
      if (obj && typeof obj === "object") {
        const out: Record<string, any> = {};
        for (const [k, v] of Object.entries(obj)) {
          out[k.startsWith(":") ? k.slice(1) : k] = stripColonKeys(v);
        }
        return out;
      }
      return obj;
    }

    // Build the request body — messages + any per-call options
    const body = JSON.stringify({
      messages: stripColonKeys(messages),
      ...stripColonKeys(streamOpts && typeof streamOpts === "object" ? streamOpts : {}),
      stream: true,
    });

    const accumulator = createStreamAccumulator((state) => {
      s.value = state;
    });

    const managedStream = openSseStream({
      url: `${proxyUrl}/stream`,
      method: "POST",
      headers: proxyHeaders,
      body,
      onEvent: (event) => accumulator.event(event.data),
      onError: (error) => accumulator.fail(error.message),
      onClose: () => {
        unregisterStream(ctx, id);
        accumulator.close();
      },
    });

    registerStream(ctx, id, {
      kind: "llm-stream",
      close: managedStream.close,
    });
    const ownerId = getCurrentOwnerId(ctx);
    const owner = ownerId != null ? ctx.mountedComponentsById.get(ownerId) ?? null : null;
    if (owner) owner.ownedStreamIds.add(id);

    return id;
  });

  interp.registerFunction("__llm/close-stream", (signalId: number) => {
    const stream = ctx.streams.get(signalId);
    if (stream) {
      stream.close();
      unregisterStream(ctx, signalId);
      for (const component of ctx.mountedComponents.values()) {
        component.ownedStreamIds.delete(signalId);
      }
    }

    const current = ctx.signals.get(signalId) as any;
    if (current) {
      // Always emit the full `{text, done, error}` shape. Spreading whatever
      // was there could yield a bare `{done: true}` for a signal that never
      // received a frame, and Sema code doing `(:text (deref s))` would get
      // nil instead of the empty string it gets everywhere else.
      const previous: Partial<LlmStreamState> = current.value ?? {};
      current.value = {
        text: typeof previous.text === "string" ? previous.text : "",
        done: true,
        error: previous.error ?? null,
      } satisfies LlmStreamState;
    }
    return null;
  });

  const semaCode = buildLlmProxySema(proxyUrl, headersMap);

  const result = interp.evalStr(semaCode);
  if (result.error) {
    throw new Error(`[sema-web] Failed to register LLM bindings: ${result.error}`);
  }
}

/**
 * Escape a string for safe embedding in Sema code.
 * Handles backslashes, double quotes, newlines, carriage returns, and tabs.
 */
function escapeSemaString(s: string): string {
  return s
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r")
    .replace(/\t/g, "\\t");
}

/**
 * Build the Sema half of the proxy client.
 *
 * Extracted from `registerLlmBindings` so it can be validated on its own. This
 * is ~75 lines of Sema embedded in a TypeScript string: nothing in the type
 * checker, the bundler, or the unit suite can see a typo in it, and the only
 * previous signal was a thrown error at `SemaWeb.create()` time in a browser.
 * `llm-proxy-sema.test.ts` now loads the output into the real `sema` binary.
 *
 * @param proxyUrl - Base URL, trailing slashes already stripped.
 * @param headersMap - A Sema map literal, pre-built by {@link buildProxyHeaders}.
 */
export function buildLlmProxySema(proxyUrl: string, headersMap: string): string {
  return `
;; --- LLM proxy internals ---

(define __llm-proxy-url "${escapeSemaString(proxyUrl)}")
(define __llm-proxy-headers ${headersMap})

;; Normalize a raw http/* response into decoded JSON, or raise.
;;
;; Both transports must fail the same way. Previously this returned the decoded
;; body regardless of status, so a proxy 500 with a JSON error body came back
;; as an ordinary value and (llm/chat ...) handed the caller an error map
;; where it expected a string — the failure surfaced later, somewhere else, as
;; a type confusion. Streaming already reported errors properly through the
;; signal's :error field; this brings the request path in line.
(define (__llm-proxy-normalize endpoint resp)
  (if (not (map? resp))
    resp
    (let ((status (or (:status resp) 200))
          (body (:body resp)))
      (cond
        ((>= status 400)
          (error (format "llm proxy ~a failed: HTTP ~a~a" endpoint status
                   (if (and (string? body) (> (string-length body) 0))
                     (format " - ~a" body)
                     ""))))
        ((not (string? body)) resp)
        ((= (string-length body) 0) {})
        (else
          (try
            (json/decode body)
            (catch e
              (error (format "llm proxy ~a returned a body that is not valid JSON"
                       endpoint)))))))))

;; Helper: POST to the proxy and decode the JSON response body.
(define (__llm-proxy-post endpoint body-map)
  (let ((url (string-append __llm-proxy-url "/" endpoint)))
    (__llm-proxy-normalize endpoint
      (http/post url
        {:headers __llm-proxy-headers
         :body (json/encode body-map)}))))

;; Helper: GET from the proxy.
(define (__llm-proxy-get endpoint)
  (let ((url (string-append __llm-proxy-url "/" endpoint)))
    (__llm-proxy-normalize endpoint
      (http/get url {:headers __llm-proxy-headers}))))

;; --- Message normalization ---
;;
;; (message :user "hi") is a SPECIAL FORM in the Sema core, and a special form
;; always wins in operator position - see limitations.md #36. A
;; (define (message ...)) here therefore never takes effect; this module used to
;; ship one anyway, so every caller silently got the core form instead. That
;; form produces a :message value, and json/encode refuses to encode it
;; ("cannot encode message as JSON"), which broke the documented usage
;; (llm/chat (list (message :user "Hi"))) outright.
;;
;; So: don't fight the special form, convert its output. Every request path
;; funnels messages through here before encoding.
(define (__llm-message->map m)
  (if (= (type-of m) :message)
    {:role (message/role m) :content (message/content m)}
    m))

(define (__llm-normalize-messages messages)
  (if (list? messages)
    (map __llm-message->map messages)
    (__llm-message->map messages)))

;; --- Public API ---

;; (llm/complete prompt) or (llm/complete prompt opts)
;; Send a simple prompt for completion.
(define (llm/complete prompt . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:prompt prompt} (if (map? opts) opts {}))))
      (let ((result (__llm-proxy-post "complete" body)))
        (if (map? result)
          (or (:content result) (:text result) result)
          result)))))

;; (llm/chat messages) or (llm/chat messages opts)
;; Chat with a list of message maps.
(define (llm/chat messages . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:messages (__llm-normalize-messages messages)}
                  (if (map? opts) opts {}))))
      (let ((result (__llm-proxy-post "chat" body)))
        (if (map? result)
          (or (:content result) (:text result) result)
          result)))))

;; (llm/send prompt) or (llm/send prompt opts)
;; Send a prompt (list of messages or prompt object).
(define (llm/send prompt . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((messages (if (list? prompt) prompt (list prompt))))
      (let ((body (merge {:messages (__llm-normalize-messages messages)}
                    (if (map? opts) opts {}))))
        (let ((result (__llm-proxy-post "chat" body)))
          (if (map? result)
            (or (:content result) (:text result) result)
            result))))))

;; (llm/extract schema text) or (llm/extract schema text opts)
;; Extract structured data from text.
(define (llm/extract schema text . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:schema schema :text text} (if (map? opts) opts {}))))
      (__llm-proxy-post "extract" body))))

;; (llm/classify categories text) or (llm/classify categories text opts)
;; Classify text into one of the given categories.
(define (llm/classify categories text . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:categories categories :text text} (if (map? opts) opts {}))))
      (__llm-proxy-post "classify" body))))

;; (llm/embed text) or (llm/embed text opts)
;; Get text embeddings.
(define (llm/embed text . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:text text} (if (map? opts) opts {}))))
      (__llm-proxy-post "embed" body))))

;; (llm/list-models)
;; List available models from the proxy.
(define (llm/list-models)
  (__llm-proxy-get "models"))

;; (llm/chat-stream messages) or (llm/chat-stream messages opts)
;; Streaming chat — returns a signal ID that updates as tokens arrive.
;; Signal value shape: {:text "" :done false :error nil}
(define (llm/chat-stream messages . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (__llm/chat-stream-raw
      (json/encode (__llm-normalize-messages messages))
      (json/encode (if (map? opts) opts {})))))

(define (llm/close-stream signal-id)
  (__llm/close-stream signal-id))
`;
}

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
import { getCurrentOwnerId } from "./context.js";
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
   * Request timeout in milliseconds.
   * Default: 60000 (60 seconds).
   */
  timeout?: number;
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

  // Build the headers map as a Sema literal expression
  const headerPairs: string[] = [];
  headerPairs.push(`"Content-Type" "application/json"`);
  if (opts.token) {
    headerPairs.push(`"Authorization" "Bearer ${escapeSemaString(opts.token)}"`);
  }
  if (opts.headers) {
    for (const [k, v] of Object.entries(opts.headers)) {
      headerPairs.push(`"${escapeSemaString(k)}" "${escapeSemaString(v)}"`);
    }
  }
  const headersMap = `{${headerPairs.join(" ")}}`;

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
    const s = signal<{ text: string; done: boolean; error: string | null }>({
      text: "",
      done: false,
      error: null,
    });
    ctx.signals.set(id, s as any);

    // Build request headers
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...(opts.headers || {}),
    };
    if (opts.token) {
      headers["Authorization"] = `Bearer ${opts.token}`;
    }

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

    let accumulated = "";
    const managedStream = openSseStream({
      url: `${proxyUrl}/stream`,
      method: "POST",
      headers,
      body,
      onEvent: (event) => {
        if (!event.data) return;
        try {
          const parsed = JSON.parse(event.data);
          if (parsed.type === "token" && typeof parsed.text === "string") {
            accumulated += parsed.text;
            s.value = { text: accumulated, done: false, error: null };
            return;
          }
          if (parsed.type === "done") {
            s.value = { text: accumulated, done: true, error: null };
            return;
          }
          if (parsed.type === "error") {
            s.value = {
              text: accumulated,
              done: true,
              error: typeof parsed.error === "string" ? parsed.error : "Stream error",
            };
          }
        } catch {
          s.value = { text: accumulated, done: true, error: "Invalid stream payload" };
        }
      },
      onError: (error) => {
        s.value = { text: accumulated, done: true, error: error.message };
      },
      onClose: () => {
        ctx.streams.delete(id);
        s.value = {
          text: accumulated,
          done: true,
          error: s.value.error,
        };
      },
    });

    ctx.streams.set(id, {
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
      ctx.streams.delete(signalId);
      for (const component of ctx.mountedComponents.values()) {
        component.ownedStreamIds.delete(signalId);
      }
    }

    const current = ctx.signals.get(signalId) as any;
    if (current) {
      current.value = {
        ...(current.value ?? {}),
        done: true,
      };
    }
    return null;
  });

  // Define all LLM proxy functions as Sema code.
  // These use http/post which the WASM async loop intercepts for fetch().
  const semaCode = `
;; --- LLM proxy internals ---

(define __llm-proxy-url "${escapeSemaString(proxyUrl)}")
(define __llm-proxy-headers ${headersMap})

;; Helper: POST to the proxy and decode the JSON response body.
(define (__llm-proxy-post endpoint body-map)
  (let ((url (string-append __llm-proxy-url "/" endpoint))
        (resp (http/post url
                {:headers __llm-proxy-headers
                 :body (json/encode body-map)})))
    (if (and (map? resp) (:body resp))
      (json/decode (:body resp))
      resp)))

;; Helper: GET from the proxy.
(define (__llm-proxy-get endpoint)
  (let ((url (string-append __llm-proxy-url "/" endpoint))
        (resp (http/get url {:headers __llm-proxy-headers})))
    (if (and (map? resp) (:body resp))
      (json/decode (:body resp))
      resp)))

;; --- Public API ---

;; (message role content) — build a chat message map
(define (message role content)
  {:role (if (keyword? role) (keyword->string role) (->string role))
   :content content})

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
    (let ((body (merge {:messages messages} (if (map? opts) opts {}))))
      (let ((result (__llm-proxy-post "chat" body)))
        (if (map? result)
          (or (:content result) (:text result) result)
          result)))))

;; (llm/send prompt) or (llm/send prompt opts)
;; Send a prompt (list of messages or prompt object).
(define (llm/send prompt . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((messages (if (list? prompt) prompt (list prompt))))
      (let ((body (merge {:messages messages} (if (map? opts) opts {}))))
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
      (json/encode messages)
      (json/encode (if (map? opts) opts {})))))

(define (llm/close-stream signal-id)
  (__llm/close-stream signal-id))
`;

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

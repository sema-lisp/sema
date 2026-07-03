/**
 * Core LLM proxy handler — platform-agnostic request processing.
 *
 * This module implements the proxy protocol defined by `@sema-lang/sema-web`.
 * Each adapter (Vercel, Netlify, Cloudflare, Node) converts platform-specific
 * request/response objects to `ProxyRequest`/`ProxyResponse` and delegates here.
 *
 * @module
 */

import type {
  ProxyConfig,
  ProxyRequest,
  ProxyResponse,
  ChatRequest,
  CompleteRequest,
  ExtractRequest,
  ClassifyRequest,
  EmbedRequest,
  ProviderConfig,
  ProxyErrorResponse,
  RateLimitConfig,
} from "./types.js";
import { byteLength } from "./body.js";
import {
  resolveProvider,
  getSpec,
  geminiChatUrl,
  geminiEmbedUrl,
  type ResolvedProvider,
} from "./providers.js";

/** Handler function type — takes a ProxyRequest, returns a ProxyResponse. */
export type HandlerFn = (req: ProxyRequest) => Promise<ProxyResponse>;

// --- Rate Limiter ---

/** Sliding-window in-memory rate limiter. */
class RateLimiter {
  private windows = new Map<string, number[]>();
  private windowMs: number;
  private maxRequests: number;

  constructor(config?: RateLimitConfig) {
    this.windowMs = config?.windowMs ?? 60_000;
    this.maxRequests = config?.maxRequests ?? 60;
  }

  check(key: string): ProxyErrorResponse | null {
    const now = Date.now();
    const timestamps = this.windows.get(key) ?? [];

    // Remove expired entries
    const valid = timestamps.filter(t => now - t < this.windowMs);

    if (valid.length >= this.maxRequests) {
      return {
        error: "Rate limit exceeded",
        code: "RATE_LIMITED",
        details: `Max ${this.maxRequests} requests per ${this.windowMs}ms`,
      };
    }

    valid.push(now);
    this.windows.set(key, valid);

    // Periodic cleanup of stale keys
    if (this.windows.size > 10_000) {
      for (const [k, v] of this.windows) {
        if (v.every(t => now - t >= this.windowMs)) this.windows.delete(k);
      }
    }

    return null;
  }
}

// --- Body size check ---

const DEFAULT_UPSTREAM_TIMEOUT_MS = 30_000;

class UpstreamTimeoutError extends Error {
  constructor(timeoutMs: number) {
    super(`Upstream provider timed out after ${timeoutMs}ms`);
    this.name = "UpstreamTimeoutError";
  }
}

/** Check request body against maxBodySize. Returns error response or null. */
function checkBodySize(body: string | object, config: ProxyConfig): ProxyErrorResponse | null {
  const maxSize = config.maxBodySize || 1_048_576; // 1MB default
  const serialized = typeof body === "string" ? body : JSON.stringify(body);
  const size = byteLength(serialized);
  if (size > maxSize) {
    return {
      error: `Request body too large (${size} bytes, max ${maxSize})`,
      code: "BODY_TOO_LARGE",
    };
  }
  return null;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isOptionalString(value: unknown): value is string | undefined {
  return value === undefined || typeof value === "string";
}

function isOptionalNumber(value: unknown): value is number | undefined {
  return value === undefined || typeof value === "number";
}

function isChatRequestBody(body: unknown): body is ChatRequest {
  if (!isPlainObject(body) || !Array.isArray(body.messages)) return false;
  if (!body.messages.every((message) =>
    isPlainObject(message)
    && typeof message.role === "string"
    && typeof message.content === "string"
  )) {
    return false;
  }

  return isOptionalString(body.model)
    && isOptionalNumber(body["max-tokens"])
    && isOptionalNumber(body.temperature)
    && isOptionalString(body.system);
}

function isCompleteRequestBody(body: unknown): body is CompleteRequest {
  return isPlainObject(body)
    && typeof body.prompt === "string"
    && isOptionalString(body.model)
    && isOptionalNumber(body["max-tokens"])
    && isOptionalNumber(body.temperature)
    && isOptionalString(body.system);
}

function isExtractRequestBody(body: unknown): body is ExtractRequest {
  return isPlainObject(body)
    && isPlainObject(body.schema)
    && typeof body.text === "string"
    && isOptionalString(body.model)
    && isOptionalNumber(body["max-tokens"]);
}

function isClassifyRequestBody(body: unknown): body is ClassifyRequest {
  return isPlainObject(body)
    && Array.isArray(body.categories)
    && body.categories.every((category) => typeof category === "string")
    && typeof body.text === "string"
    && isOptionalString(body.model);
}

function isEmbedRequestBody(body: unknown): body is EmbedRequest {
  return isPlainObject(body)
    && typeof body.text === "string"
    && isOptionalString(body.model);
}

function validateRequest(req: ProxyRequest): ProxyErrorResponse | null {
  switch (req.endpoint) {
    case "chat":
    case "stream":
      return isChatRequestBody(req.body)
        ? null
        : {
            error: "Invalid chat request body",
            code: "INVALID_REQUEST",
            details: "Expected { messages: [{ role, content }], model?, max-tokens?, temperature?, system? }",
          };
    case "complete":
      return isCompleteRequestBody(req.body)
        ? null
        : {
            error: "Invalid complete request body",
            code: "INVALID_REQUEST",
            details: "Expected { prompt: string, model?, max-tokens?, temperature?, system? }",
          };
    case "extract":
      return isExtractRequestBody(req.body)
        ? null
        : {
            error: "Invalid extract request body",
            code: "INVALID_REQUEST",
            details: "Expected { schema: object, text: string, model?, max-tokens? }",
          };
    case "classify":
      return isClassifyRequestBody(req.body)
        ? null
        : {
            error: "Invalid classify request body",
            code: "INVALID_REQUEST",
            details: "Expected { categories: string[], text: string, model? }",
          };
    case "embed":
      return isEmbedRequestBody(req.body)
        ? null
        : {
            error: "Invalid embed request body",
            code: "INVALID_REQUEST",
            details: "Expected { text: string, model? }",
          };
    case "models":
      return req.body == null
        ? null
        : {
            error: "Invalid models request body",
            code: "INVALID_REQUEST",
            details: "GET /models does not accept a request body",
          };
    default:
      return null;
  }
}

function isTimeoutError(error: unknown): error is UpstreamTimeoutError {
  return error instanceof UpstreamTimeoutError;
}

async function fetchWithTimeout(
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<Response> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);

  try {
    return await fetch(url, {
      ...init,
      signal: controller.signal,
    });
  } catch (error) {
    if (controller.signal.aborted) {
      throw new UpstreamTimeoutError(timeoutMs);
    }
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

async function readWithIdleTimeout<T>(
  read: Promise<T>,
  timeoutMs: number,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | null = null;
  try {
    return await Promise.race([
      read,
      new Promise<T>((_, reject) => {
        timer = setTimeout(() => reject(new UpstreamTimeoutError(timeoutMs)), timeoutMs);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

function encodeSseEvent(payload: Record<string, unknown>): Uint8Array {
  return new TextEncoder().encode(`data: ${JSON.stringify(payload)}\n\n`);
}

function providerStreamMode(provider: ResolvedProvider["provider"]): "openai" | "anthropic" | "fallback" {
  if (provider === "anthropic") return "anthropic";
  if (provider === "gemini") return "fallback";
  return "openai";
}

function extractNormalizedToken(
  mode: "openai" | "anthropic" | "fallback",
  parsed: Record<string, any>,
): { type: "token"; text: string } | { type: "done" } | { type: "error"; error: string } | null {
  if (mode === "anthropic") {
    if (parsed.type === "message_stop") return { type: "done" };
    if (parsed.type === "error") {
      return { type: "error", error: String(parsed.error?.message ?? "Provider stream error") };
    }
    const text = parsed.delta?.text;
    return typeof text === "string" && text.length > 0 ? { type: "token", text } : null;
  }

  if (parsed.error) {
    return { type: "error", error: String(parsed.error?.message ?? parsed.error) };
  }

  const text =
    parsed.choices?.[0]?.delta?.content
    ?? parsed.choices?.[0]?.message?.content
    ?? parsed.message?.content
    ?? parsed.content;

  return typeof text === "string" && text.length > 0 ? { type: "token", text } : null;
}

/**
 * Create a platform-agnostic handler function from a proxy config.
 *
 * The returned function accepts `ProxyRequest` and returns `ProxyResponse`.
 * Adapters wrap this to convert platform-specific types.
 */
export function createHandler(config: ProxyConfig): HandlerFn {
  const resolved = resolveProvider(
    config.provider,
    config.apiKey,
    config.baseUrl,
    config.defaultModel,
  );
  const corsOrigin = config.cors ?? "*";
  const rateLimiter = new RateLimiter(config.rateLimit);
  const upstreamTimeoutMs = config.upstreamTimeoutMs ?? DEFAULT_UPSTREAM_TIMEOUT_MS;

  return async (req: ProxyRequest): Promise<ProxyResponse> => {
    // CORS preflight
    if (req.method === "OPTIONS") {
      return corsResponse(corsOrigin, config.corsAllowedHeaders, req.requestedHeaders);
    }

    // Auth check
    const authResult = await checkAuth(config.auth, req.authHeader);
    if (authResult) {
      return {
        ...authResult,
        headers: {
          ...authResult.headers,
          ...corsHeaders(corsOrigin, config.corsAllowedHeaders, req.requestedHeaders),
        },
      };
    }

    // Rate limit check
    const rateLimitKey = req.clientId || req.authHeader || "anonymous";
    const rateLimitError = rateLimiter.check(rateLimitKey);
    if (rateLimitError) {
      return jsonResponse(429, rateLimitError, corsOrigin, config.corsAllowedHeaders, req.requestedHeaders);
    }

    // Body size check
    if (req.body != null) {
      const bodySizeError = checkBodySize(req.body as string | object, config);
      if (bodySizeError) {
        return jsonResponse(413, bodySizeError, corsOrigin, config.corsAllowedHeaders, req.requestedHeaders);
      }
    }

    const validationError = validateRequest(req);
    if (validationError) {
      return jsonResponse(400, validationError, corsOrigin, config.corsAllowedHeaders, req.requestedHeaders);
    }

    try {
      let result: unknown;

      switch (req.endpoint) {
        case "chat":
          result = await handleChat(resolved, req.body as ChatRequest, upstreamTimeoutMs);
          break;
        case "complete":
          result = await handleComplete(resolved, req.body as CompleteRequest, upstreamTimeoutMs);
          break;
        case "extract":
          result = await handleExtract(resolved, req.body as ExtractRequest, upstreamTimeoutMs);
          break;
        case "classify":
          result = await handleClassify(resolved, req.body as ClassifyRequest, upstreamTimeoutMs);
          break;
        case "embed":
          result = await handleEmbed(resolved, req.body as EmbedRequest, upstreamTimeoutMs);
          break;
        case "models":
          result = await handleModels(resolved, upstreamTimeoutMs);
          break;
        case "stream":
          return await handleStream(resolved, req.body as ChatRequest, corsOrigin, upstreamTimeoutMs);
        default:
          return jsonResponse(404, {
            error: `Unknown endpoint: ${req.endpoint}`,
            code: "INVALID_REQUEST" as const,
          }, corsOrigin, config.corsAllowedHeaders, req.requestedHeaders);
      }

      return jsonResponse(200, result, corsOrigin, config.corsAllowedHeaders, req.requestedHeaders);
    } catch (err) {
      if (isTimeoutError(err)) {
        return jsonResponse(504, {
          error: err.message,
          code: "TIMEOUT" as const,
        }, corsOrigin, config.corsAllowedHeaders, req.requestedHeaders);
      }
      const message = err instanceof Error ? err.message : String(err);
      return jsonResponse(502, {
        error: message,
        code: "PROVIDER_ERROR" as const,
      }, corsOrigin, config.corsAllowedHeaders, req.requestedHeaders);
    }
  };
}

// --- Endpoint handlers ---

async function handleChat(
  provider: ResolvedProvider,
  body: ChatRequest,
  timeoutMs: number,
): Promise<unknown> {
  const spec = getSpec(provider.provider);
  const model = body.model ?? provider.defaultModel;
  const maxTokens = body["max-tokens"];

  const reqBody = spec.formatChatBody(
    body.messages,
    model,
    maxTokens,
    body.temperature,
    body.system,
  );

  const url =
    provider.provider === "gemini"
      ? geminiChatUrl(provider.baseUrl, model, provider.apiKey)
      : spec.chatUrl(provider.baseUrl);

  const data = await llmFetch(url, reqBody, spec.authHeader(provider.apiKey), timeoutMs);
  return spec.parseChatResponse(data);
}

async function handleComplete(
  provider: ResolvedProvider,
  body: CompleteRequest,
  timeoutMs: number,
): Promise<unknown> {
  // Complete is implemented as a single-message chat
  return handleChat(provider, {
    messages: [{ role: "user", content: body.prompt }],
    model: body.model,
    "max-tokens": body["max-tokens"],
    temperature: body.temperature,
    system: body.system,
  }, timeoutMs);
}

async function handleExtract(
  provider: ResolvedProvider,
  body: ExtractRequest,
  timeoutMs: number,
): Promise<unknown> {
  const schemaStr = JSON.stringify(body.schema, null, 2);
  const systemPrompt = [
    "Extract structured data from the text below.",
    "Return ONLY a JSON object matching this schema — no extra text:",
    schemaStr,
  ].join("\n");

  const response = await handleChat(provider, {
    messages: [{ role: "user", content: body.text }],
    model: body.model,
    "max-tokens": body["max-tokens"] ?? 1024,
    system: systemPrompt,
  }, timeoutMs);

  // Try to parse the content as JSON
  const content =
    typeof response === "object" && response !== null && "content" in response
      ? (response as { content: string }).content
      : String(response);

  try {
    return JSON.parse(extractJsonBlock(content));
  } catch {
    return { content, _raw: true };
  }
}

async function handleClassify(
  provider: ResolvedProvider,
  body: ClassifyRequest,
  timeoutMs: number,
): Promise<unknown> {
  const cats = body.categories.map((c) => `"${c}"`).join(", ");
  const systemPrompt = [
    `Classify the text into exactly ONE of these categories: ${cats}`,
    "Respond with ONLY the category name — no explanation, no quotes.",
  ].join("\n");

  const response = await handleChat(provider, {
    messages: [{ role: "user", content: body.text }],
    model: body.model,
    "max-tokens": 50,
    system: systemPrompt,
  }, timeoutMs);

  const content =
    typeof response === "object" && response !== null && "content" in response
      ? (response as { content: string }).content.trim()
      : String(response).trim();

  return { category: content };
}

async function handleEmbed(
  provider: ResolvedProvider,
  body: EmbedRequest,
  timeoutMs: number,
): Promise<unknown> {
  const spec = getSpec(provider.provider);
  const model = body.model ?? spec.embedModel ?? provider.defaultModel;

  const url =
    provider.provider === "gemini"
      ? geminiEmbedUrl(provider.baseUrl, model, provider.apiKey)
      : spec.embedUrl(provider.baseUrl);

  const reqBody = spec.formatEmbedBody(body.text, model);
  const data = await llmFetch(url, reqBody, spec.authHeader(provider.apiKey), timeoutMs);
  return spec.parseEmbedResponse(data);
}

async function handleModels(provider: ResolvedProvider, timeoutMs: number): Promise<unknown> {
  const spec = getSpec(provider.provider);

  // For Gemini, model listing uses a different URL format
  if (provider.provider === "gemini") {
    const url = `${provider.baseUrl}/models?key=${provider.apiKey}`;
    const data = await llmFetch(url, null, {}, timeoutMs);
    const models = data.models as Array<{ name: string }>;
    return {
      models: models?.map((m) => m.name.replace("models/", "")) ?? [],
    };
  }

  // For Ollama and OpenAI-compatible providers
  const url = `${provider.baseUrl}/models`;
  try {
    const data = await llmFetch(url, null, spec.authHeader(provider.apiKey), timeoutMs);
    const items = data.data as Array<{ id: string }>;
    return {
      models: items?.map((m) => m.id) ?? [],
    };
  } catch {
    // Fallback: return the default model
    return { models: [provider.defaultModel] };
  }
}

// --- SSE Streaming ---

/** Handle a streaming chat request, returning an SSE response. */
async function handleStream(
  provider: ResolvedProvider,
  body: ChatRequest,
  corsOrigin: string,
  timeoutMs: number,
): Promise<ProxyResponse> {
  if (providerStreamMode(provider.provider) === "fallback") {
    const response = await handleChat(provider, body, timeoutMs) as { content?: string } | string;
    const content =
      typeof response === "object" && response !== null && "content" in response
        ? String(response.content ?? "")
        : String(response ?? "");

    return {
      status: 200,
      headers: {
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache",
        "Connection": "keep-alive",
        // Prevent reverse proxies (nginx, and others that honor this
        // convention) from buffering the stream before flushing to the client.
        "X-Accel-Buffering": "no",
        ...corsHeaders(corsOrigin),
      },
      body: "",
      stream: new ReadableStream<Uint8Array>({
        start(controller) {
          if (content) controller.enqueue(encodeSseEvent({ type: "token", text: content }));
          controller.enqueue(encodeSseEvent({ type: "done" }));
          controller.close();
        },
      }),
    };
  }

  const spec = getSpec(provider.provider);
  const model = body.model ?? provider.defaultModel;
  const messages = body.messages || [{ role: "user", content: "" }];

  const chatBody = spec.formatChatBody(
    messages,
    model,
    body["max-tokens"],
    body.temperature,
    body.system,
  );
  (chatBody as Record<string, unknown>).stream = true;

  const url =
    provider.provider === "gemini"
      ? geminiChatUrl(provider.baseUrl, model, provider.apiKey)
      : spec.chatUrl(provider.baseUrl);

  const headers = { ...spec.authHeader(provider.apiKey), "Content-Type": "application/json" };

  const response = await fetchWithTimeout(url, {
    method: "POST",
    headers,
    body: JSON.stringify(chatBody),
  }, timeoutMs);

  if (!response.ok || !response.body) {
    const text = await response.text().catch(() => response.statusText);
    const errorBody: ProxyErrorResponse = {
      error: `Provider streaming error (${response.status}): ${text}`,
      code: "PROVIDER_ERROR",
    };
    return jsonResponse(502, errorBody, corsOrigin);
  }

  const mode = providerStreamMode(provider.provider);
  const upstream = response.body;
  return {
    status: 200,
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      "Connection": "keep-alive",
      // Prevent reverse proxies (nginx, and others that honor this
      // convention) from buffering the stream before flushing to the client.
      "X-Accel-Buffering": "no",
      ...corsHeaders(corsOrigin),
    },
    body: "",
    stream: new ReadableStream<Uint8Array>({
      async start(controller) {
        const reader = upstream.getReader();
        const decoder = new TextDecoder();
        let buffer = "";

        try {
          while (true) {
            const { done, value } = await readWithIdleTimeout(reader.read(), timeoutMs);
            if (done) break;

            buffer += decoder.decode(value, { stream: true });
            const lines = buffer.split("\n");
            buffer = lines.pop() || "";

            for (const line of lines) {
              if (!line.startsWith("data: ")) continue;
              const data = line.slice(6).trim();
              if (data === "[DONE]") {
                controller.enqueue(encodeSseEvent({ type: "done" }));
                controller.close();
                return;
              }

              try {
                const parsed = JSON.parse(data) as Record<string, any>;
                const normalized = extractNormalizedToken(mode, parsed);
                if (!normalized) continue;
                controller.enqueue(encodeSseEvent(normalized));
                if (normalized.type === "done" || normalized.type === "error") {
                  controller.close();
                  return;
                }
              } catch {
                controller.enqueue(encodeSseEvent({ type: "token", text: data }));
              }
            }
          }

          controller.enqueue(encodeSseEvent({ type: "done" }));
          controller.close();
        } catch (error) {
          if (isTimeoutError(error)) {
            reader.cancel().catch(() => {});
          }
          const message = isTimeoutError(error)
            ? error.message
            : (error instanceof Error ? error.message : String(error));
          controller.enqueue(encodeSseEvent({ type: "error", error: message }));
          controller.close();
        } finally {
          reader.releaseLock();
        }
      },
      cancel() {
        upstream.cancel().catch(() => {});
      },
    }),
  };
}

// --- Utilities ---

/** Make an HTTP request to an LLM provider API. */
async function llmFetch(
  url: string,
  body: unknown,
  headers: Record<string, string>,
  timeoutMs: number,
): Promise<Record<string, unknown>> {
  const isGet = body === null;
  const reqHeaders: Record<string, string> = {
    ...headers,
  };

  if (!isGet) {
    reqHeaders["Content-Type"] = "application/json";
  }

  const resp = await fetchWithTimeout(url, {
    method: isGet ? "GET" : "POST",
    headers: reqHeaders,
    body: isGet ? undefined : JSON.stringify(body),
  }, timeoutMs);

  if (!resp.ok) {
    const text = await resp.text().catch(() => resp.statusText);
    throw new Error(`LLM API error (${resp.status}): ${text}`);
  }

  return (await resp.json()) as Record<string, unknown>;
}

/** Extract a JSON block from a string (handles ```json ... ``` wrapping). */
function extractJsonBlock(s: string): string {
  // Strip markdown code blocks
  const match = s.match(/```(?:json)?\s*\n?([\s\S]*?)\n?\s*```/);
  if (match) return match[1].trim();
  // Try to find a raw JSON object/array
  const idx = s.indexOf("{");
  if (idx >= 0) return s.slice(idx);
  return s;
}

/** Check authorization. Returns an error response if unauthorized, null if OK. */
async function checkAuth(
  auth: ProxyConfig["auth"],
  authHeader: string | null,
): Promise<ProxyResponse | null> {
  if (!auth) return null;

  if (auth.verify) {
    const ok = await auth.verify(authHeader);
    if (!ok) {
      return jsonResponse(401, { error: "Unauthorized", code: "AUTH_FAILED" as const }, "*");
    }
    return null;
  }

  if (auth.token) {
    const expected = `Bearer ${auth.token}`;
    if (authHeader !== expected) {
      return jsonResponse(401, { error: "Unauthorized", code: "AUTH_FAILED" as const }, "*");
    }
  }

  return null;
}

/** Build a JSON response. */
function jsonResponse(
  status: number,
  body: unknown,
  corsOrigin: string,
  allowedHeaders?: string[],
  requestedHeaders?: string | null,
): ProxyResponse {
  return {
    status,
    headers: {
      "Content-Type": "application/json",
      ...corsHeaders(corsOrigin, allowedHeaders, requestedHeaders),
    },
    body: JSON.stringify(body),
  };
}

/** CORS headers. */
function corsHeaders(
  origin: string,
  allowedHeaders?: string[],
  requestedHeaders?: string | null,
): Record<string, string> {
  const headerNames = new Set<string>(["Content-Type", "Authorization"]);
  for (const header of allowedHeaders ?? []) {
    const trimmed = header.trim();
    if (trimmed) headerNames.add(trimmed);
  }
  for (const header of (requestedHeaders ?? "").split(",")) {
    const trimmed = header.trim();
    if (trimmed) headerNames.add(trimmed);
  }
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
    "Access-Control-Allow-Headers": Array.from(headerNames).join(", "),
  };
}

/** CORS preflight response. */
function corsResponse(
  origin: string,
  allowedHeaders?: string[],
  requestedHeaders?: string | null,
): ProxyResponse {
  return {
    status: 204,
    headers: corsHeaders(origin, allowedHeaders, requestedHeaders),
    body: "",
  };
}

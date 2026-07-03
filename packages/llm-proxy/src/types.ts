/**
 * Types for the LLM proxy server.
 *
 * These types define the protocol between `@sema-lang/sema-web` (browser)
 * and the proxy server (this package).
 *
 * @module
 */

// --- Provider configuration ---

/** Supported LLM provider identifiers. */
export type ProxyProvider =
  | "openai"
  | "anthropic"
  | "gemini"
  | "ollama"
  | "groq"
  | "mistral"
  | "xai";

/** Configuration for a single LLM provider. */
export interface ProviderConfig {
  /** Provider identifier. */
  provider: ProxyProvider;

  /** API key for the provider (required for cloud providers). */
  apiKey?: string;

  /** Override the base URL (for Ollama, Azure OpenAI, proxies, etc.). */
  baseUrl?: string;

  /** Default model to use if not specified in the request. */
  defaultModel?: string;
}

/** Authentication configuration for the proxy itself. */
export interface AuthConfig {
  /**
   * Shared secret token. If set, requests must include
   * `Authorization: Bearer {token}` to be accepted.
   */
  token?: string;

  /**
   * Custom auth function. Receives the `Authorization` header value.
   * Return `true` to allow, `false` to reject.
   */
  verify?: (authHeader: string | null) => boolean | Promise<boolean>;
}

/** Top-level proxy configuration. */
export interface ProxyConfig {
  /** Primary LLM provider config. */
  provider: ProxyProvider | ProviderConfig;

  /** API key (shorthand — used when `provider` is a string). */
  apiKey?: string;

  /** Base URL override (shorthand — used when `provider` is a string). */
  baseUrl?: string;

  /** Default model (shorthand — used when `provider` is a string). */
  defaultModel?: string;

  /** Authentication for incoming requests from the browser. */
  auth?: AuthConfig;

  /**
   * CORS origin. Set to `"*"` for any origin, or a specific origin string.
   * Default: `"*"`.
   */
  cors?: string;

  /**
   * Additional headers allowed in CORS preflight responses.
   * Use this when browser clients send custom headers to the proxy.
   */
  corsAllowedHeaders?: string[];

  /**
   * Maximum request body size in bytes.
   * Default: 1MB (1_048_576).
   */
  maxBodySize?: number;

  /** Rate limiting configuration. */
  rateLimit?: RateLimitConfig;

  /**
   * Timeout for upstream provider requests in milliseconds.
   * Applies to both streaming and non-streaming provider calls.
   * Default: 30000.
   */
  upstreamTimeoutMs?: number;

  /**
   * Whether proxy forwarding headers should be trusted for client identity.
   *
   * - `false` / unset: do not trust proxy forwarding headers
   * - `true`: trust the default forwarding headers
   * - `string[]`: trust the provided header names, in order
   */
  trustProxyHeaders?: boolean | string[];
}

// --- Request/Response types (protocol between browser and proxy) ---

/** Chat message in the proxy protocol. */
export interface ChatMessage {
  role: string;
  content: string;
}

/** POST /chat request body. */
export interface ChatRequest {
  messages: ChatMessage[];
  model?: string;
  "max-tokens"?: number;
  temperature?: number;
  system?: string;
}

/** POST /chat response body. */
export interface ChatResponse {
  content: string;
  model?: string;
  usage?: {
    "prompt-tokens"?: number;
    "completion-tokens"?: number;
  };
}

/** POST /complete request body. */
export interface CompleteRequest {
  prompt: string;
  model?: string;
  "max-tokens"?: number;
  temperature?: number;
  system?: string;
}

/** POST /complete response body. */
export interface CompleteResponse {
  content: string;
  model?: string;
  usage?: {
    "prompt-tokens"?: number;
    "completion-tokens"?: number;
  };
}

/** POST /extract request body. */
export interface ExtractRequest {
  schema: Record<string, unknown>;
  text: string;
  model?: string;
  "max-tokens"?: number;
}

/** POST /classify request body. */
export interface ClassifyRequest {
  categories: string[];
  text: string;
  model?: string;
}

/** POST /embed request body. */
export interface EmbedRequest {
  text: string;
  model?: string;
}

/** POST /embed response body. */
export interface EmbedResponse {
  embedding: number[];
  model?: string;
}

/** GET /models response body. */
export interface ModelsResponse {
  models: string[];
}

/** Platform-agnostic incoming request. */
export interface ProxyRequest {
  /** HTTP method (uppercase). */
  method: string;
  /** URL path after the base prefix (e.g. "chat", "complete"). */
  endpoint: string;
  /** Parsed JSON body (null for GET). */
  body: unknown;
  /** Authorization header value (or null). */
  authHeader: string | null;
  /** Best-effort client identity for rate limiting (IP/session/etc.). */
  clientId?: string | null;
  /** Browser-requested CORS headers from preflight, if any. */
  requestedHeaders?: string | null;
}

/** Platform-agnostic outgoing response. */
export interface ProxyResponse {
  status: number;
  headers: Record<string, string>;
  body: string;
  /** Optional readable stream for SSE — when present, adapters should pipe it directly instead of using `body`. */
  stream?: ReadableStream<Uint8Array>;
}

// --- Error types ---

/** Structured error codes returned by the proxy. */
export type ProxyErrorCode =
  | "AUTH_FAILED"
  | "RATE_LIMITED"
  | "PROVIDER_ERROR"
  | "INVALID_REQUEST"
  | "TIMEOUT"
  | "BODY_TOO_LARGE";

/** Structured error response body. */
export interface ProxyErrorResponse {
  error: string;
  code: ProxyErrorCode;
  details?: string;
}

// --- Rate limiting ---

/** Configuration for the sliding-window rate limiter. */
export interface RateLimitConfig {
  /** Time window in milliseconds. Default: 60000 (1 minute). */
  windowMs?: number;
  /** Maximum requests per window. Default: 60. */
  maxRequests?: number;
}

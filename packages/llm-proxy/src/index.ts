/**
 * @sema-lang/llm-proxy — Server-side LLM proxy for Sema web apps.
 *
 * This package provides the backend proxy that `@sema-lang/sema-web`
 * connects to when you configure `llmProxy`. It forwards LLM requests
 * from the browser to actual LLM providers (OpenAI, Anthropic, etc.)
 * while keeping API keys secure on the server.
 *
 * ## Quick Start
 *
 * ```ts
 * // Vercel Edge Functions (app/api/llm/[...path]/route.ts)
 * import { createVercelHandler } from "@sema-lang/llm-proxy/vercel";
 * export const { GET, POST } = createVercelHandler({
 *   provider: "openai",
 *   apiKey: process.env.OPENAI_API_KEY!,
 * });
 * ```
 *
 * ## Architecture
 *
 * ```
 * Browser (sema-web)              This package               LLM Provider
 * ┌────────────────┐         ┌──────────────────┐         ┌──────────────┐
 * │ llm/chat       │──POST──▶│ /chat handler    │──POST──▶│ OpenAI API   │
 * │ llm/complete   │──POST──▶│ /complete        │──POST──▶│ Anthropic    │
 * │ llm/embed      │──POST──▶│ /embed           │──POST──▶│ Gemini       │
 * └────────────────┘         └──────────────────┘         └──────────────┘
 * ```
 *
 * @module
 */

export { createHandler } from "./handler.js";
export type {
  ProxyConfig,
  ProxyProvider,
  ProviderConfig,
  AuthConfig,
  ChatRequest,
  ChatResponse,
  CompleteRequest,
  CompleteResponse,
  ExtractRequest,
  ClassifyRequest,
  EmbedRequest,
  EmbedResponse,
  ModelsResponse,
  ProxyRequest,
  ProxyResponse,
  ProxyErrorCode,
  ProxyErrorResponse,
  RateLimitConfig,
} from "./types.js";

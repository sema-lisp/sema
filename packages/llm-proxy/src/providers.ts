/**
 * LLM provider client implementations.
 *
 * Each provider knows how to translate the proxy protocol into
 * the provider's native API format and back.
 *
 * @module
 */

import type {
  ProviderConfig,
  ProxyProvider,
  ChatMessage,
  ChatResponse,
  EmbedResponse,
} from "./types.js";

/** Normalized provider configuration. */
export interface ResolvedProvider {
  provider: ProxyProvider;
  apiKey: string;
  baseUrl: string;
  defaultModel: string;
}

/** Provider endpoint URLs and request formats. */
interface ProviderSpec {
  chatUrl: (base: string) => string;
  embedUrl: (base: string) => string;
  defaultModel: string;
  embedModel: string;
  formatChatBody: (
    messages: ChatMessage[],
    model: string,
    maxTokens?: number,
    temperature?: number,
    system?: string,
  ) => Record<string, unknown>;
  parseChatResponse: (data: Record<string, unknown>) => ChatResponse;
  formatEmbedBody: (text: string, model: string) => Record<string, unknown>;
  parseEmbedResponse: (data: Record<string, unknown>) => EmbedResponse;
  authHeader: (apiKey: string) => Record<string, string>;
}

// --- Provider specs ---

const openaiSpec: ProviderSpec = {
  chatUrl: (base) => `${base}/chat/completions`,
  embedUrl: (base) => `${base}/embeddings`,
  defaultModel: "gpt-4o-mini",
  embedModel: "text-embedding-3-small",
  formatChatBody: (messages, model, maxTokens, temperature, system) => {
    const msgs = system
      ? [{ role: "system", content: system }, ...messages]
      : messages;
    const body: Record<string, unknown> = { model, messages: msgs };
    if (maxTokens) body.max_tokens = maxTokens;
    if (temperature !== undefined) body.temperature = temperature;
    return body;
  },
  parseChatResponse: (data) => {
    const choices = data.choices as Array<{ message: { content: string } }>;
    const usage = data.usage as
      | { prompt_tokens: number; completion_tokens: number }
      | undefined;
    return {
      content: choices?.[0]?.message?.content ?? "",
      model: data.model as string,
      usage: usage
        ? {
            "prompt-tokens": usage.prompt_tokens,
            "completion-tokens": usage.completion_tokens,
          }
        : undefined,
    };
  },
  formatEmbedBody: (text, model) => ({
    model,
    input: text,
  }),
  parseEmbedResponse: (data) => {
    const embedData = data.data as Array<{ embedding: number[] }>;
    return {
      embedding: embedData?.[0]?.embedding ?? [],
      model: data.model as string,
    };
  },
  authHeader: (apiKey) => ({ Authorization: `Bearer ${apiKey}` }),
};

const anthropicSpec: ProviderSpec = {
  chatUrl: (base) => `${base}/messages`,
  embedUrl: () => {
    throw new Error("Anthropic does not support embeddings");
  },
  defaultModel: "claude-sonnet-4-20250514",
  embedModel: "",
  formatChatBody: (messages, model, maxTokens, temperature, system) => {
    const body: Record<string, unknown> = {
      model,
      messages,
      max_tokens: maxTokens ?? 1024,
    };
    if (temperature !== undefined) body.temperature = temperature;
    if (system) body.system = system;
    return body;
  },
  parseChatResponse: (data) => {
    const content = data.content as Array<{ type: string; text: string }>;
    const usage = data.usage as
      | { input_tokens: number; output_tokens: number }
      | undefined;
    return {
      content: content?.find((b) => b.type === "text")?.text ?? "",
      model: data.model as string,
      usage: usage
        ? {
            "prompt-tokens": usage.input_tokens,
            "completion-tokens": usage.output_tokens,
          }
        : undefined,
    };
  },
  formatEmbedBody: () => {
    throw new Error("Anthropic does not support embeddings");
  },
  parseEmbedResponse: () => {
    throw new Error("Anthropic does not support embeddings");
  },
  authHeader: (apiKey) => ({
    "x-api-key": apiKey,
    "anthropic-version": "2023-06-01",
  }),
};

const geminiSpec: ProviderSpec = {
  chatUrl: () => "",
  embedUrl: () => "",
  defaultModel: "gemini-2.0-flash",
  embedModel: "text-embedding-004",
  formatChatBody: (messages, _model, maxTokens, temperature, system) => {
    const contents = messages.map((m) => ({
      role: m.role === "assistant" ? "model" : "user",
      parts: [{ text: m.content }],
    }));
    const body: Record<string, unknown> = { contents };
    const config: Record<string, unknown> = {};
    if (maxTokens) config.maxOutputTokens = maxTokens;
    if (temperature !== undefined) config.temperature = temperature;
    if (Object.keys(config).length > 0) body.generationConfig = config;
    if (system)
      body.systemInstruction = { parts: [{ text: system }] };
    return body;
  },
  parseChatResponse: (data) => {
    const candidates = data.candidates as Array<{
      content: { parts: Array<{ text: string }> };
    }>;
    const usage = data.usageMetadata as
      | { promptTokenCount: number; candidatesTokenCount: number }
      | undefined;
    return {
      content: candidates?.[0]?.content?.parts?.[0]?.text ?? "",
      usage: usage
        ? {
            "prompt-tokens": usage.promptTokenCount,
            "completion-tokens": usage.candidatesTokenCount,
          }
        : undefined,
    };
  },
  formatEmbedBody: (text) => ({
    content: { parts: [{ text }] },
  }),
  parseEmbedResponse: (data) => {
    const embedding = data.embedding as { values: number[] };
    return {
      embedding: embedding?.values ?? [],
    };
  },
  authHeader: () => ({}),
};

// Groq, Mistral, xAI all use the OpenAI-compatible API format
const groqSpec: ProviderSpec = {
  ...openaiSpec,
  defaultModel: "llama-3.3-70b-versatile",
  embedModel: "",
};

const mistralSpec: ProviderSpec = {
  ...openaiSpec,
  defaultModel: "mistral-small-latest",
  embedModel: "mistral-embed",
};

const xaiSpec: ProviderSpec = {
  ...openaiSpec,
  defaultModel: "grok-3-mini",
  embedModel: "",
};

const ollamaSpec: ProviderSpec = {
  ...openaiSpec,
  defaultModel: "llama3.2",
  embedModel: "nomic-embed-text",
  authHeader: () => ({}),
};

// --- Provider registry ---

const PROVIDER_SPECS: Record<ProxyProvider, ProviderSpec> = {
  openai: openaiSpec,
  anthropic: anthropicSpec,
  gemini: geminiSpec,
  ollama: ollamaSpec,
  groq: groqSpec,
  mistral: mistralSpec,
  xai: xaiSpec,
};

const DEFAULT_BASE_URLS: Record<ProxyProvider, string> = {
  openai: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com/v1",
  gemini: "https://generativelanguage.googleapis.com/v1beta",
  ollama: "http://localhost:11434/v1",
  groq: "https://api.groq.com/openai/v1",
  mistral: "https://api.mistral.ai/v1",
  xai: "https://api.x.ai/v1",
};

/** Resolve a provider config into a normalized form. */
export function resolveProvider(
  config: ProxyProvider | ProviderConfig,
  apiKey?: string,
  baseUrl?: string,
  defaultModel?: string,
): ResolvedProvider {
  const providerConfig: ProviderConfig =
    typeof config === "string" ? { provider: config } : config;

  const provider = providerConfig.provider;
  const spec = PROVIDER_SPECS[provider];
  if (!spec) {
    throw new Error(`Unknown LLM provider: ${provider}`);
  }

  return {
    provider,
    apiKey: providerConfig.apiKey ?? apiKey ?? "",
    baseUrl: (
      providerConfig.baseUrl ??
      baseUrl ??
      DEFAULT_BASE_URLS[provider]
    ).replace(/\/+$/, ""),
    defaultModel: providerConfig.defaultModel ?? defaultModel ?? spec.defaultModel,
  };
}

/** Get the spec for a provider. */
export function getSpec(provider: ProxyProvider): ProviderSpec {
  return PROVIDER_SPECS[provider];
}

/** Build the URL for a Gemini chat request (needs API key in URL). */
export function geminiChatUrl(
  baseUrl: string,
  model: string,
  apiKey: string,
): string {
  return `${baseUrl}/models/${model}:generateContent?key=${apiKey}`;
}

/** Build the URL for a Gemini embed request. */
export function geminiEmbedUrl(
  baseUrl: string,
  model: string,
  apiKey: string,
): string {
  return `${baseUrl}/models/${model}:embedContent?key=${apiKey}`;
}

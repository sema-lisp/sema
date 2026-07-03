/**
 * Local LLM proxy server for Sema Web development.
 *
 * Usage:
 *   npx tsx examples/web-demo/proxy.ts
 *
 * Uses ANTHROPIC_API_KEY from environment by default.
 * Change `provider` below to use openai, ollama, etc.
 */
import { createServer } from "node:http";
import { createNodeHandler } from "../../packages/llm-proxy/src/adapters/node.js";

const provider = process.env.LLM_PROVIDER || "anthropic";
const port = parseInt(process.env.PORT || "3002");

const handler = createNodeHandler({
  provider: provider as any,
  apiKey: process.env.ANTHROPIC_API_KEY || process.env.OPENAI_API_KEY || "",
  defaultModel:
    provider === "anthropic"
      ? "claude-haiku-4-5-20251001"
      : provider === "openai"
        ? "gpt-4o-mini"
        : "granite4",
  cors: "*",
  rateLimit: { windowMs: 60_000, maxRequests: 30 },
});

createServer((req, res) => {
  // Health check for Playwright webServer readiness
  if (req.method === "GET" && req.url === "/health") {
    res.writeHead(200);
    res.end("ok");
    return;
  }
  handler(req, res);
}).listen(port, () => {
  console.log(`LLM proxy running on http://localhost:${port} (provider: ${provider})`);
});

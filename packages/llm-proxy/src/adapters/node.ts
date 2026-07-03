/**
 * Generic Node.js HTTP adapter for the Sema LLM proxy.
 *
 * Works with any Node.js HTTP framework that uses the standard
 * `http.IncomingMessage` / `http.ServerResponse` types, including
 * Express, Fastify, Hono, Koa, and plain `http.createServer`.
 *
 * ## Usage with Express
 *
 * ```ts
 * import express from "express";
 * import { createNodeHandler } from "@sema-lang/llm-proxy/node";
 *
 * const app = express();
 *
 * const llmProxy = createNodeHandler({
 *   provider: "openai",
 *   apiKey: process.env.OPENAI_API_KEY!,
 * });
 *
 * // Mount at any prefix. Express 5 (the current default) requires named
 * // wildcards — a bare "*" throws "Missing parameter name" at startup.
 * // On Express 4 use "/api/llm/*" instead.
 * app.all("/api/llm/*splat", llmProxy);
 *
 * app.listen(3001);
 * ```
 *
 * ## Usage with plain http.createServer
 *
 * ```ts
 * import { createServer } from "http";
 * import { createNodeHandler } from "@sema-lang/llm-proxy/node";
 *
 * const handler = createNodeHandler({
 *   provider: "openai",
 *   apiKey: process.env.OPENAI_API_KEY!,
 * });
 *
 * createServer(handler).listen(3001);
 * ```
 *
 * Then in your frontend:
 * ```js
 * await SemaWeb.create({ llmProxy: "http://localhost:3001/api/llm" });
 * ```
 *
 * @module
 */

import { createHandler } from "../handler.js";
import { extractClientIdFromNodeHeaders } from "../client-id.js";
import { buildBodyTooLargeResponse, getMaxBodySize } from "../body.js";
import type { ProxyConfig, ProxyRequest } from "../types.js";

/** Minimal Node.js IncomingMessage interface. */
interface NodeRequest {
  method?: string;
  url?: string;
  headers: Record<string, string | string[] | undefined>;
  on(event: "data", listener: (chunk: Uint8Array) => void): void;
  on(event: "end", listener: () => void): void;
  on(event: "error", listener: (err: Error) => void): void;
}

/** Minimal Node.js ServerResponse interface. */
interface NodeResponse {
  writeHead(status: number, headers: Record<string, string>): void;
  write(chunk: string | Uint8Array): boolean;
  end(body?: string): void;
  /** Not every framework's response wrapper exposes this — call defensively. */
  flushHeaders?(): void;
}

/** Node.js HTTP handler function. */
export type NodeHandler = (
  req: NodeRequest,
  res: NodeResponse,
) => void;

/**
 * Create a Node.js HTTP handler for the LLM proxy.
 *
 * Compatible with Express, Fastify, Koa, Hono, plain `http.createServer`, etc.
 */
export function createNodeHandler(config: ProxyConfig): NodeHandler {
  const handler = createHandler(config);
  const corsOrigin = config.cors ?? "*";
  const maxBodySize = getMaxBodySize(config);

  return (req: NodeRequest, res: NodeResponse): void => {
    const url = req.url ?? "/";
    const endpoint = extractEndpoint(url);

    // Read request body
    if (req.method === "POST") {
      const contentLength = getHeader(req.headers, "content-length");
      if (contentLength) {
        const declaredBytes = Number.parseInt(contentLength, 10);
        if (Number.isFinite(declaredBytes) && declaredBytes > maxBodySize) {
          const tooLarge = buildBodyTooLargeResponse(corsOrigin, maxBodySize, declaredBytes);
          res.writeHead(tooLarge.status, tooLarge.headers);
          res.end(tooLarge.body);
          return;
        }
      }

      const chunks: Uint8Array[] = [];
      let totalBytes = 0;
      let tooLarge = false;
      req.on("data", (chunk) => {
        totalBytes += chunk.length;
        if (totalBytes > maxBodySize) {
          tooLarge = true;
          return;
        }
        chunks.push(chunk);
      });
      req.on("end", () => {
        if (tooLarge) {
          const response = buildBodyTooLargeResponse(corsOrigin, maxBodySize, totalBytes);
          res.writeHead(response.status, response.headers);
          res.end(response.body);
          return;
        }

        const bodyStr = new TextDecoder().decode(concatUint8Arrays(chunks));
        let body: unknown = null;
        try {
          body = JSON.parse(bodyStr);
        } catch {
          res.writeHead(400, {
            "Content-Type": "application/json",
            "Access-Control-Allow-Origin": corsOrigin,
            "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
            "Access-Control-Allow-Headers": "Content-Type, Authorization",
          });
          res.end(JSON.stringify({ error: "Invalid JSON body", code: "INVALID_REQUEST" }));
          return;
        }

        const proxyReq: ProxyRequest = {
          method: "POST",
          endpoint,
          body,
          authHeader: getHeader(req.headers, "authorization"),
          clientId: extractClientIdFromNodeHeaders(
            (name) => getHeader(req.headers, name),
            config.trustProxyHeaders,
          ),
          requestedHeaders: getHeader(req.headers, "access-control-request-headers"),
        };

        handler(proxyReq).then(
          async (proxyRes) => {
            res.writeHead(proxyRes.status, proxyRes.headers);
            // If the response has a ReadableStream, pipe it directly for SSE
            if (proxyRes.stream) {
              // Send headers immediately rather than letting Node buffer them
              // until the first flush-worthy write — SSE clients (and any
              // reverse proxy in front of them) should see the connection
              // open right away.
              res.flushHeaders?.();
              const reader = proxyRes.stream.getReader();
              try {
                while (true) {
                  const { done, value } = await reader.read();
                  if (done) break;
                  res.write(value);
                }
              } finally {
                reader.releaseLock();
                res.end();
              }
            } else {
              res.end(proxyRes.body);
            }
          },
          (err) => {
            res.writeHead(500, { "Content-Type": "application/json" });
            res.end(JSON.stringify({ error: String(err) }));
          },
        );
      });
      req.on("error", (err) => {
        res.writeHead(500, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: String(err) }));
      });
    } else {
      const proxyReq: ProxyRequest = {
        method: req.method ?? "GET",
        endpoint,
        body: null,
        authHeader: getHeader(req.headers, "authorization"),
        clientId: extractClientIdFromNodeHeaders(
          (name) => getHeader(req.headers, name),
          config.trustProxyHeaders,
        ),
        requestedHeaders: getHeader(req.headers, "access-control-request-headers"),
      };

      handler(proxyReq).then(
        (proxyRes) => {
          res.writeHead(proxyRes.status, proxyRes.headers);
          res.end(proxyRes.body);
        },
        (err) => {
          res.writeHead(500, { "Content-Type": "application/json" });
          res.end(JSON.stringify({ error: String(err) }));
        },
      );
    }
  };
}

/**
 * Extract the LLM endpoint from the request URL.
 * e.g. "/api/llm/chat" → "chat"
 */
function extractEndpoint(url: string): string {
  const path = url.split("?")[0] ?? "";
  const segments = path.split("/").filter(Boolean);
  return segments[segments.length - 1] ?? "";
}

/** Get a header value from Node.js headers (handles arrays). */
function getHeader(
  headers: Record<string, string | string[] | undefined>,
  name: string,
): string | null {
  const val = headers[name] ?? headers[name.toLowerCase()];
  if (Array.isArray(val)) return val[0] ?? null;
  return val ?? null;
}

/** Concatenate Uint8Array chunks into a single array. */
function concatUint8Arrays(chunks: Uint8Array[]): Uint8Array {
  let totalLength = 0;
  for (const chunk of chunks) totalLength += chunk.length;
  const result = new Uint8Array(totalLength);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
}

// Re-export config types for convenience
export type { ProxyConfig } from "../types.js";

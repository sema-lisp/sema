import type { ProxyConfig, ProxyResponse } from "./types.js";

const textEncoder = new TextEncoder();

export function getMaxBodySize(config: ProxyConfig): number {
  return config.maxBodySize ?? 1_048_576;
}

export function buildBodyTooLargeResponse(
  corsOrigin: string,
  maxBytes: number,
  size?: number,
): ProxyResponse {
  const message = size != null
    ? `Request body too large (${size} bytes, max ${maxBytes})`
    : `Request body too large (max ${maxBytes} bytes)`;

  return {
    status: 413,
    headers: {
      "Content-Type": "application/json",
      "Access-Control-Allow-Origin": corsOrigin,
      "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
      "Access-Control-Allow-Headers": "Content-Type, Authorization",
    },
    body: JSON.stringify({
      error: message,
      code: "BODY_TOO_LARGE",
    }),
  };
}

export async function readRequestTextWithLimit(
  req: Request,
  maxBytes: number,
): Promise<{ ok: true; text: string } | { ok: false; size: number }> {
  const contentLength = req.headers.get("content-length");
  if (contentLength) {
    const parsed = Number.parseInt(contentLength, 10);
    if (Number.isFinite(parsed) && parsed > maxBytes) {
      return { ok: false, size: parsed };
    }
  }

  if (!req.body) {
    return { ok: true, text: "" };
  }

  const reader = req.body.getReader();
  const decoder = new TextDecoder();
  let totalBytes = 0;
  let text = "";

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      totalBytes += value.byteLength;
      if (totalBytes > maxBytes) {
        await reader.cancel().catch(() => {});
        return { ok: false, size: totalBytes };
      }
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
    return { ok: true, text };
  } finally {
    reader.releaseLock();
  }
}

export function byteLength(value: string): number {
  return textEncoder.encode(value).length;
}

function buildInvalidJsonResponse(corsOrigin: string): ProxyResponse {
  return {
    status: 400,
    headers: {
      "Content-Type": "application/json",
      "Access-Control-Allow-Origin": corsOrigin,
      "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
      "Access-Control-Allow-Headers": "Content-Type, Authorization",
    },
    body: JSON.stringify({ error: "Invalid JSON body", code: "INVALID_REQUEST" }),
  };
}

/**
 * Read and JSON-parse a POST body under a size limit, for adapters built on
 * the standard Fetch API Request/Response shape (Vercel, Cloudflare,
 * Netlify). The Node adapter reads from an IncomingMessage stream instead
 * and doesn't share this helper.
 */
export async function parseJsonBody(
  req: Request,
  corsOrigin: string,
  maxBytes: number,
): Promise<{ ok: true; body: unknown } | { ok: false; response: ProxyResponse }> {
  try {
    const bodyResult = await readRequestTextWithLimit(req, maxBytes);
    if (!bodyResult.ok) {
      return { ok: false, response: buildBodyTooLargeResponse(corsOrigin, maxBytes, bodyResult.size) };
    }
    return { ok: true, body: bodyResult.text ? JSON.parse(bodyResult.text) : null };
  } catch {
    return { ok: false, response: buildInvalidJsonResponse(corsOrigin) };
  }
}

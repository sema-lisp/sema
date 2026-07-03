import http from "node:http";

let cannedResponse = { content: "Mock response", role: "assistant" };
const requests: any[] = [];

/**
 * Build a deterministic reply for `/stream` (llm/chat-stream) requests,
 * derived entirely from the incoming user message. Keeping this stateless
 * (no shared mutable "canned reply" config) avoids cross-test races when
 * Playwright specs run in parallel against this single shared server.
 */
function buildStreamReply(userText: string): string {
  if (userText.includes("Generate 3 NEW tasks")) {
    return JSON.stringify([
      { title: "Set up staging environment", priority: "medium" },
      { title: "Add integration test suite", priority: "high" },
      { title: "Write onboarding docs", priority: "low" },
    ]);
  }
  return `Mock reply to: ${userText}`;
}

/** Split text into word+trailing-space chunks so streamed tokens concatenate cleanly. */
function chunkText(text: string): string[] {
  const parts = text.match(/\S+\s*/g);
  return parts && parts.length > 0 ? parts : [text];
}

const server = http.createServer((req, res) => {
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Headers", "Content-Type,Authorization");
  res.setHeader("Access-Control-Allow-Methods", "GET,POST,OPTIONS");

  if (req.method === "OPTIONS") { res.writeHead(204); res.end(); return; }

  if (req.url === "/health") { res.writeHead(200); res.end("ok"); return; }

  if (req.url === "/mock-proxy/set-response") {
    let body = "";
    req.on("data", chunk => body += chunk);
    req.on("end", () => {
      cannedResponse = JSON.parse(body);
      res.writeHead(200); res.end("ok");
    });
    return;
  }

  if (req.url === "/mock-proxy/requests") {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(requests));
    return;
  }

  // Streaming chat endpoint used by llm/chat-stream (sema-web's SSE client).
  // Echoes back a deterministic, request-derived reply as real chunked SSE
  // (via res.write + setTimeout) so tests can observe incremental rendering.
  if (req.url === "/stream" && req.method === "POST") {
    let body = "";
    req.on("data", chunk => body += chunk);
    req.on("end", () => {
      let parsed: any = {};
      try { parsed = JSON.parse(body || "{}"); } catch { /* ignore malformed body */ }
      const messages = Array.isArray(parsed.messages) ? parsed.messages : [];
      const lastUser = [...messages].reverse().find((m: any) => m && m.role === "user");
      const userText = typeof lastUser?.content === "string" ? lastUser.content : "";

      res.writeHead(200, {
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache",
        "Connection": "keep-alive",
      });

      const tokens = chunkText(buildStreamReply(userText));
      let i = 0;
      const sendNext = () => {
        if (i >= tokens.length) {
          res.write(`data: ${JSON.stringify({ type: "done" })}\n\n`);
          res.end();
          return;
        }
        res.write(`data: ${JSON.stringify({ type: "token", text: tokens[i] })}\n\n`);
        i += 1;
        setTimeout(sendNext, 30);
      };
      sendNext();
    });
    return;
  }

  // Record request
  let body = "";
  req.on("data", chunk => body += chunk);
  req.on("end", () => {
    requests.push({ url: req.url, method: req.method, body: JSON.parse(body || "{}"), headers: req.headers });
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(cannedResponse));
  });
});

const port = parseInt(process.argv.find(a => a.startsWith("--port="))?.split("=")[1] || "3002");
server.listen(port, () => console.log(`Mock proxy on :${port}`));

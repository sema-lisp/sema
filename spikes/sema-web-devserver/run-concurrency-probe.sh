#!/usr/bin/env bash
# Empirically test whether an in-flight SSE stream blocks a concurrent plain
# request on http/serve. If /hello returns in ~0s while /sse is mid-stream, the
# server is concurrent. If /hello only returns after /sse finishes (~3s), the
# server is sequential (single evaluator, handlers run inline).
set -uo pipefail
cd "$(dirname "$0")/../.."

SEMA=target/release/sema
PORT=3011

"$SEMA" spikes/sema-web-devserver/concurrency-probe.sema &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null' EXIT
sleep 1  # let it bind

echo "== firing SSE (long) request in background =="
curl -sN "http://127.0.0.1:$PORT/sse" >/tmp/sse-out.txt &
SSE_PID=$!
sleep 0.3  # ensure SSE is mid-stream

echo "== timing a concurrent /hello request while SSE is open =="
START=$(python3 -c 'import time; print(time.time())')
HELLO=$(curl -s "http://127.0.0.1:$PORT/hello")
END=$(python3 -c 'import time; print(time.time())')
ELAPSED=$(python3 -c "print(f'{$END - $START:.2f}')")

wait $SSE_PID 2>/dev/null
echo
echo "hello body:    '$HELLO'"
echo "hello latency: ${ELAPSED}s   (≈0s => CONCURRENT,  ≈2.7s => SEQUENTIAL/BLOCKED)"
echo "sse body:"
sed 's/^/  /' /tmp/sse-out.txt

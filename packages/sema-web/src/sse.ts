export interface SseEvent {
  data: string;
  event: string | null;
  id: string | null;
  retry: number | null;
}

export interface OpenSseStreamOptions {
  url: string;
  method?: string;
  headers?: Record<string, string>;
  body?: string;
  credentials?: RequestCredentials;
  signal?: AbortSignal;
  onOpen?: (response: Response) => void;
  onEvent: (event: SseEvent) => void;
  onError?: (error: Error) => void;
  onClose?: () => void;
}

export interface ManagedSseStream {
  close: () => void;
  done: Promise<void>;
}

interface EventBuffer {
  data: string[];
  event: string | null;
  id: string | null;
  retry: number | null;
}

function createEventBuffer(): EventBuffer {
  return {
    data: [],
    event: null,
    id: null,
    retry: null,
  };
}

function emitBufferedEvent(
  buffer: EventBuffer,
  onEvent: (event: SseEvent) => void,
): void {
  if (
    buffer.data.length === 0
    && buffer.event == null
    && buffer.id == null
    && buffer.retry == null
  ) {
    return;
  }

  onEvent({
    data: buffer.data.join("\n"),
    event: buffer.event,
    id: buffer.id,
    retry: buffer.retry,
  });
}

export function openSseStream(opts: OpenSseStreamOptions): ManagedSseStream {
  const controller = new AbortController();
  const forwardedSignal = opts.signal;
  const abortForwarder = () => controller.abort(forwardedSignal?.reason);
  if (forwardedSignal) {
    if (forwardedSignal.aborted) {
      controller.abort(forwardedSignal.reason);
    } else {
      forwardedSignal.addEventListener("abort", abortForwarder, { once: true });
    }
  }

  const done = (async () => {
    try {
      const response = await fetch(opts.url, {
        method: opts.method ?? (opts.body != null ? "POST" : "GET"),
        headers: opts.headers,
        body: opts.body,
        credentials: opts.credentials,
        signal: controller.signal,
      });

      if (!response.ok || !response.body) {
        throw new Error(`HTTP ${response.status}`);
      }

      opts.onOpen?.(response);

      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let textBuffer = "";
      let eventBuffer = createEventBuffer();

      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;

          textBuffer += decoder.decode(value, { stream: true });
          const lines = textBuffer.split(/\r?\n/);
          textBuffer = lines.pop() ?? "";

          for (const line of lines) {
            if (line === "") {
              emitBufferedEvent(eventBuffer, opts.onEvent);
              eventBuffer = createEventBuffer();
              continue;
            }

            if (line.startsWith(":")) continue;

            const separator = line.indexOf(":");
            const field = separator === -1 ? line : line.slice(0, separator);
            let rawValue = separator === -1 ? "" : line.slice(separator + 1);
            if (rawValue.startsWith(" ")) rawValue = rawValue.slice(1);

            switch (field) {
              case "data":
                eventBuffer.data.push(rawValue);
                break;
              case "event":
                eventBuffer.event = rawValue || null;
                break;
              case "id":
                eventBuffer.id = rawValue || null;
                break;
              case "retry": {
                const parsed = Number.parseInt(rawValue, 10);
                eventBuffer.retry = Number.isFinite(parsed) ? parsed : null;
                break;
              }
              default:
                break;
            }
          }
        }

        textBuffer += decoder.decode();
        const finalLines = textBuffer.split(/\r?\n/);
        for (const line of finalLines) {
          if (!line) continue;
          if (line.startsWith(":")) continue;
          const separator = line.indexOf(":");
          const field = separator === -1 ? line : line.slice(0, separator);
          let rawValue = separator === -1 ? "" : line.slice(separator + 1);
          if (rawValue.startsWith(" ")) rawValue = rawValue.slice(1);
          if (field === "data") eventBuffer.data.push(rawValue);
          else if (field === "event") eventBuffer.event = rawValue || null;
          else if (field === "id") eventBuffer.id = rawValue || null;
          else if (field === "retry") {
            const parsed = Number.parseInt(rawValue, 10);
            eventBuffer.retry = Number.isFinite(parsed) ? parsed : null;
          }
        }
        emitBufferedEvent(eventBuffer, opts.onEvent);
      } finally {
        reader.releaseLock();
      }
    } catch (error) {
      if (!controller.signal.aborted) {
        opts.onError?.(error instanceof Error ? error : new Error(String(error)));
      }
    } finally {
      if (forwardedSignal) {
        forwardedSignal.removeEventListener("abort", abortForwarder);
      }
      opts.onClose?.();
    }
  })();

  return {
    close: () => controller.abort(),
    done,
  };
}

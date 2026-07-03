const DEFAULT_PROXY_HEADERS = [
  "cf-connecting-ip",
  "x-forwarded-for",
  "x-real-ip",
] as const;

function parseForwardedFor(value: string | null | undefined): string | null {
  if (!value) return null;
  const first = value.split(",")[0]?.trim();
  return first || null;
}

function normalizeClientId(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}

function resolveTrustedHeaderList(
  trustProxyHeaders: boolean | string[] | undefined,
  defaultTrust: boolean,
): string[] {
  if (Array.isArray(trustProxyHeaders)) {
    return trustProxyHeaders.map((name) => name.toLowerCase());
  }
  if (trustProxyHeaders === true) {
    return [...DEFAULT_PROXY_HEADERS];
  }
  if (trustProxyHeaders === false) {
    return [];
  }
  return defaultTrust ? [...DEFAULT_PROXY_HEADERS] : [];
}

export function extractClientIdFromNodeHeaders(
  getHeader: (name: string) => string | null,
  trustProxyHeaders?: boolean | string[],
): string | null {
  const trustedHeaders = resolveTrustedHeaderList(trustProxyHeaders, false);
  for (const header of trustedHeaders) {
    const value = getHeader(header);
    if (header === "x-forwarded-for") {
      const parsed = parseForwardedFor(value);
      if (parsed) return parsed;
      continue;
    }
    const normalized = normalizeClientId(value);
    if (normalized) return normalized;
  }
  return null;
}

export function extractClientIdFromRequestHeaders(
  headers: Pick<Headers, "get">,
  trustProxyHeaders?: boolean | string[],
): string | null {
  const trustedHeaders = resolveTrustedHeaderList(trustProxyHeaders, true);
  for (const header of trustedHeaders) {
    const value = headers.get(header);
    if (header === "x-forwarded-for") {
      const parsed = parseForwardedFor(value);
      if (parsed) return parsed;
      continue;
    }
    const normalized = normalizeClientId(value);
    if (normalized) return normalized;
  }
  return null;
}

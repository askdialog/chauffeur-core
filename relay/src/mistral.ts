// Only ephemeral credentials pass through the relay; audio goes directly to Mistral.
const SESSION_URL = "https://api.mistral.ai/v1/client/sessions";
const MODEL = "voxtral-mini-transcribe-realtime-2602";
const MAX_RESPONSE = 16 * 1024;

type Transport = (url: string, init: RequestInit) => Promise<Response>;

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Mint only the fixed transcription model's WebSocket credential, never provider diagnostics. */
export function mistralSessions(apiKey: string, transport: Transport = fetch) {
  return async (signal: AbortSignal): Promise<string> => {
    const response = await transport(SESSION_URL, {
      method: "POST",
      headers: { Authorization: `Bearer ${apiKey}`, "Content-Type": "application/json" },
      body: JSON.stringify({ purpose: "realtime", model: MODEL }),
      redirect: "error",
      signal,
    });
    if (response.status !== 201 || !response.body) {
      await response.body?.cancel();
      throw new Error("Voice provider unavailable");
    }
    const reader = response.body.getReader();
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > MAX_RESPONSE) throw new Error("Invalid voice session");
        chunks.push(value);
      }
    } finally {
      await reader.cancel();
      reader.releaseLock();
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
    if (!record(value) || !record(value.client_secret)) throw new Error("Invalid voice session");
    const secret = value.client_secret;
    // RFC 6455 subprotocol token characters; exclude separators and bound header size.
    if (typeof secret.value !== "string" || !/^rt_[A-Za-z0-9._~-]{1,2045}$/.test(secret.value)) {
      throw new Error("Invalid voice session");
    }
    for (const expiry of [value.expires_at, secret.expires_at]) {
      if (expiry !== undefined && (typeof expiry !== "string" || !Number.isFinite(Date.parse(expiry)) || Date.parse(expiry) <= Date.now())) {
        throw new Error("Expired voice session");
      }
    }
    return secret.value;
  };
}

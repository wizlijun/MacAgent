import type { Env } from "./env";

const enc = new TextEncoder();

function b64urlEncode(input: string | Uint8Array): string {
  const bytes = typeof input === "string" ? enc.encode(input) : input;
  let s = btoa(String.fromCharCode(...bytes));
  return s.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function importPemPrivateKey(pem: string): Promise<CryptoKey> {
  const cleaned = pem.replace(/-----BEGIN PRIVATE KEY-----/, "")
    .replace(/-----END PRIVATE KEY-----/, "")
    .replace(/\s+/g, "");
  const der = Uint8Array.from(atob(cleaned), c => c.charCodeAt(0));
  return crypto.subtle.importKey(
    "pkcs8", der, { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"],
  );
}

async function signApnsJwt(env: Env): Promise<string> {
  if (!env.APNS_KEY_ID || !env.APNS_TEAM_ID || !env.APNS_AUTH_KEY) {
    throw new Error("APNS secrets not configured");
  }
  const header = { alg: "ES256", kid: env.APNS_KEY_ID, typ: "JWT" };
  const payload = { iss: env.APNS_TEAM_ID, iat: Math.floor(Date.now() / 1000) };
  const headerB64 = b64urlEncode(JSON.stringify(header));
  const payloadB64 = b64urlEncode(JSON.stringify(payload));
  const signingInput = `${headerB64}.${payloadB64}`;

  const key = await importPemPrivateKey(env.APNS_AUTH_KEY);
  const sig = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" }, key, enc.encode(signingInput),
  );
  // ECDSA signature is raw r||s (P-256: 64 bytes)
  return `${signingInput}.${b64urlEncode(new Uint8Array(sig))}`;
}

export interface ApnsPushResult {
  ok: boolean;
  status: number;
  reason?: string;
}

export async function pushApns(
  env: Env, deviceToken: string, payload: object,
): Promise<ApnsPushResult> {
  const jwt = await signApnsJwt(env);
  const isProd = env.APNS_ENV !== "sandbox";
  const host = isProd ? "api.push.apple.com" : "api.sandbox.push.apple.com";
  const res = await fetch(`https://${host}/3/device/${deviceToken}`, {
    method: "POST",
    headers: {
      authorization: `bearer ${jwt}`,
      "apns-topic": env.APNS_BUNDLE_ID!,
      "apns-push-type": "alert",
      "apns-priority": "10",
      "content-type": "application/json",
    },
    body: JSON.stringify(payload),
  });
  if (res.status === 410) return { ok: false, status: 410, reason: "unregistered" };
  if (!res.ok) {
    const errText = await res.text().catch(() => "");
    return { ok: false, status: res.status, reason: errText.slice(0, 200) };
  }
  return { ok: true, status: 200 };
}

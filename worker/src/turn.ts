import type { Env } from "./env";
import { b64decode, hmacVerify } from "./crypto";
import { getPair } from "./kv";

interface CallsCredResp {
  iceServers: Array<{ urls: string | string[]; username?: string; credential?: string }>;
}

export async function handleTurnCred(req: Request, env: Env): Promise<Response> {
  let body: { pair_id?: string; ts?: number; sig?: string };
  try {
    body = await req.json();
  } catch {
    return Response.json({ error: "invalid_json" }, { status: 400 });
  }
  if (!body.pair_id || typeof body.ts !== "number" || !body.sig) {
    return Response.json({ error: "missing_fields" }, { status: 400 });
  }
  if (Math.abs(Date.now() - body.ts) > 60_000) {
    return Response.json({ error: "ts_out_of_range" }, { status: 400 });
  }

  const pair = await getPair(env, body.pair_id);
  if (!pair) {
    return Response.json({ error: "unknown_pair" }, { status: 404 });
  }

  const msg = `turn-cred|${body.pair_id}|${body.ts}`;
  const sigBytes = b64decode(body.sig);
  const macOk = await hmacVerify(b64decode(pair.mac_device_secret_b64), msg, sigBytes);
  const iosOk = !macOk && await hmacVerify(b64decode(pair.ios_device_secret_b64), msg, sigBytes);
  if (!macOk && !iosOk) {
    return Response.json({ error: "bad_sig" }, { status: 401 });
  }

  if (!env.CF_CALLS_KEY_ID || !env.CF_CALLS_KEY_API_TOKEN) {
    return Response.json({ error: "turn_not_configured" }, { status: 503 });
  }

  const ttlSec = 3600;
  const callsRes = await fetch(
    `https://rtc.live.cloudflare.com/v1/turn/keys/${env.CF_CALLS_KEY_ID}/credentials/generate`,
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${env.CF_CALLS_KEY_API_TOKEN}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ ttl: ttlSec }),
    },
  );
  if (!callsRes.ok) {
    const errBody = await callsRes.text().catch(() => "<unreadable>");
    console.log("[turn/cred] Calls API failed", callsRes.status, errBody.slice(0, 500));
    console.log("[turn/cred] Used KEY_ID prefix:", env.CF_CALLS_KEY_ID?.slice(0, 10), "len:", env.CF_CALLS_KEY_ID?.length);
    const listRes = await fetch("https://rtc.live.cloudflare.com/v1/turn/keys", {
      headers: { Authorization: `Bearer ${env.CF_CALLS_KEY_API_TOKEN}` },
    });
    const listBody = await listRes.text().catch(() => "<unreadable>");
    console.log("[turn/cred] Available keys for this token:", listRes.status, listBody.slice(0, 800));
    return Response.json({ error: "turn_unavailable", status: callsRes.status }, { status: 503 });
  }
  const cred = (await callsRes.json()) as CallsCredResp;
  return Response.json({
    ice_servers: cred.iceServers,
    expires_at: Date.now() + ttlSec * 1000,
  });
}

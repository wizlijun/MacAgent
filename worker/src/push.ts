import type { Env } from "./env";
import { b64decode, hmacVerify } from "./crypto";
import { getPair, isApnsDead, markApnsDead } from "./kv";
import { pushApns } from "./apns";

export async function handlePush(req: Request, env: Env): Promise<Response> {
  let body: { pair_id?: string; ts?: number; sig?: string;
              title?: string; body?: string; deeplink?: string; thread_id?: string };
  try { body = await req.json(); } catch {
    return Response.json({ error: "invalid_json" }, { status: 400 });
  }
  if (!body.pair_id || typeof body.ts !== "number" || !body.sig
      || !body.title || !body.body) {
    return Response.json({ error: "missing_fields" }, { status: 400 });
  }
  if (Math.abs(Date.now() - body.ts) > 60_000) {
    return Response.json({ error: "ts_out_of_range" }, { status: 400 });
  }

  const pair = await getPair(env, body.pair_id);
  if (!pair) {
    return Response.json({ error: "unknown_pair" }, { status: 404 });
  }

  const msg = `push|${body.pair_id}|${body.ts}|${body.title}|${body.body}`;
  const sigBytes = b64decode(body.sig);
  const macOk = await hmacVerify(b64decode(pair.mac_device_secret_b64), msg, sigBytes);
  if (!macOk) {
    return Response.json({ error: "bad_sig" }, { status: 401 });
  }

  if (await isApnsDead(env, body.pair_id)) {
    return Response.json({ error: "apns_unregistered" }, { status: 410 });
  }

  const token = await env.PAIRS.get(`apns_token:${body.pair_id}`);
  if (!token) {
    return Response.json({ error: "apns_token_missing" }, { status: 410 });
  }

  if (!env.APNS_AUTH_KEY || !env.APNS_KEY_ID || !env.APNS_TEAM_ID || !env.APNS_BUNDLE_ID) {
    return Response.json({ error: "apns_not_configured" }, { status: 503 });
  }

  const result = await pushApns(env, token, {
    aps: {
      alert: { title: body.title, body: body.body },
      "thread-id": body.thread_id,
      sound: "default",
    },
    deeplink: body.deeplink,
  });

  if (result.status === 410) {
    await markApnsDead(env, body.pair_id, "unregistered");
    return Response.json({ error: "apns_unregistered" }, { status: 410 });
  }
  if (!result.ok) {
    return Response.json({ error: "apns_error", status: result.status, reason: result.reason }, { status: 503 });
  }
  return Response.json({ pushed: true });
}

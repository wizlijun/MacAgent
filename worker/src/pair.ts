import type { Env } from "./env";
import { genDeviceSecret, genPairToken, isValidX25519PubB64, hmacVerify, b64decode } from "./crypto";
import { putPairToken, getPairToken, deletePairToken, putPair, getPair, markRevoked } from "./kv";

export async function handlePairCreate(req: Request, env: Env): Promise<Response> {
  let body: { mac_pubkey?: string };
  try {
    body = await req.json();
  } catch {
    return Response.json({ error: "invalid_json" }, { status: 400 });
  }
  if (!body.mac_pubkey || !isValidX25519PubB64(body.mac_pubkey)) {
    return Response.json({ error: "invalid_mac_pubkey" }, { status: 400 });
  }

  const pair_token = genPairToken();
  const room_id = crypto.randomUUID();
  const mac_device_secret = genDeviceSecret();

  await putPairToken(env, pair_token, {
    mac_pubkey_b64: body.mac_pubkey,
    mac_device_secret_b64: mac_device_secret,
    room_id,
    expires: Date.now() + 5 * 60 * 1000,
  });

  return Response.json({ pair_token, room_id, mac_device_secret });
}

export async function handlePairClaim(req: Request, env: Env): Promise<Response> {
  let body: { pair_token?: string; ios_pubkey?: string; ios_apns_token?: string };
  try { body = await req.json(); } catch { return Response.json({ error: "invalid_json" }, { status: 400 }); }
  if (!body.pair_token || typeof body.pair_token !== "string") {
    return Response.json({ error: "missing_pair_token" }, { status: 400 });
  }
  if (!body.ios_pubkey || !isValidX25519PubB64(body.ios_pubkey)) {
    return Response.json({ error: "invalid_ios_pubkey" }, { status: 400 });
  }

  const tokRec = await getPairToken(env, body.pair_token);
  if (!tokRec) {
    return Response.json({ error: "unknown_or_expired_token" }, { status: 404 });
  }

  const pair_id = crypto.randomUUID();
  const ios_device_secret = genDeviceSecret();

  // 顺序：先 putPair（让 pair record 落地），再 deletePairToken（防止旧 token 重复使用），
  // 最后写 room_event（让 Mac Agent 端轮询能看到 iOS 已加入）。
  // 这三步非原子（KV 不支持事务）；若中途 crash 会留下未删 token，
  // 但 5 分钟 TTL 是兜底；M1.3 改用 DO 后通知机制，room_event KV 项可下线。
  await putPair(env, pair_id, {
    mac_pubkey_b64: tokRec.mac_pubkey_b64,
    ios_pubkey_b64: body.ios_pubkey,
    mac_device_secret_b64: tokRec.mac_device_secret_b64,
    ios_device_secret_b64: ios_device_secret,
    ios_apns_token: body.ios_apns_token,
    created_ts: Date.now(),
  });

  if (body.ios_apns_token) {
    await env.PAIRS.put(`apns_token:${pair_id}`, body.ios_apns_token);
  }

  await deletePairToken(env, body.pair_token);

  // 通知正在 room_id 上等待的 Mac 端
  await env.PAIRS.put(
    `room_event:${tokRec.room_id}`,
    JSON.stringify({ peer_joined: true, pair_id, ios_pubkey_b64: body.ios_pubkey }),
    { expirationTtl: 300 },
  );

  return Response.json({ pair_id, mac_pubkey: tokRec.mac_pubkey_b64, ios_device_secret });
}

export async function handlePairRevoke(req: Request, env: Env): Promise<Response> {
  let body: { pair_id?: string; ts?: number; sig?: string };
  try { body = await req.json(); } catch { return Response.json({ error: "invalid_json" }, { status: 400 }); }
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
  const msg = `revoke|${body.pair_id}|${body.ts}`;
  const sigBytes = b64decode(body.sig);
  const macOk = await hmacVerify(b64decode(pair.mac_device_secret_b64), msg, sigBytes);
  const iosOk = !macOk && await hmacVerify(b64decode(pair.ios_device_secret_b64), msg, sigBytes);
  if (!macOk && !iosOk) {
    return Response.json({ error: "bad_sig" }, { status: 401 });
  }
  await markRevoked(env, body.pair_id, "client_initiated");
  await env.PAIRS.delete(`pair:${body.pair_id}`);
  // Drop APNs state too so a re-pair under the same id can't push to a stale token.
  await env.PAIRS.delete(`apns_token:${body.pair_id}`);
  await env.PAIRS.delete(`apns_dead:${body.pair_id}`);
  return Response.json({ revoked: true });
}

export async function handlePairEvent(req: Request, env: Env, room_id: string): Promise<Response> {
  if (!room_id || !/^[a-f0-9-]{36}$/.test(room_id)) {
    return Response.json({ error: "invalid_room_id" }, { status: 400 });
  }
  const evt = await env.PAIRS.get(`room_event:${room_id}`, "json");
  if (!evt) {
    return Response.json({ error: "not_found" }, { status: 404 });
  }
  return Response.json(evt);
}

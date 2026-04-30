import type { Env } from "./env";
import { genDeviceSecret, genPairToken, isValidX25519PubB64 } from "./crypto";
import { putPairToken, getPairToken, deletePairToken, putPair } from "./kv";

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

  await putPair(env, pair_id, {
    mac_pubkey_b64: tokRec.mac_pubkey_b64,
    ios_pubkey_b64: body.ios_pubkey,
    mac_device_secret_b64: tokRec.mac_device_secret_b64,
    ios_device_secret_b64: ios_device_secret,
    ios_apns_token: body.ios_apns_token,
    created_ts: Date.now(),
  });

  await deletePairToken(env, body.pair_token);

  // 通知正在 room_id 上等待的 Mac 端
  await env.PAIRS.put(
    `room_event:${tokRec.room_id}`,
    JSON.stringify({ peer_joined: true, pair_id, ios_pubkey_b64: body.ios_pubkey }),
    { expirationTtl: 300 },
  );

  return Response.json({ pair_id, mac_pubkey: tokRec.mac_pubkey_b64, ios_device_secret });
}

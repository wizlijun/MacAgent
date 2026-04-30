import type { Env } from "./env";
import { genDeviceSecret, genPairToken, isValidX25519PubB64 } from "./crypto";
import { putPairToken } from "./kv";

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

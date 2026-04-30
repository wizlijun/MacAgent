import type { Env } from "./env";

export interface PairTokenRecord {
  mac_pubkey_b64: string;
  mac_device_secret_b64: string;
  room_id: string;
  expires: number;
}

export interface PairRecord {
  mac_pubkey_b64: string;
  ios_pubkey_b64: string;
  mac_device_secret_b64: string;
  ios_device_secret_b64: string;
  ios_apns_token?: string;
  created_ts: number;
}

export async function getPairToken(env: Env, token: string): Promise<PairTokenRecord | null> {
  return await env.PAIRS.get(`pair_token:${token}`, "json");
}

export async function putPairToken(env: Env, token: string, rec: PairTokenRecord): Promise<void> {
  await env.PAIRS.put(`pair_token:${token}`, JSON.stringify(rec), { expirationTtl: 300 });
}

export async function deletePairToken(env: Env, token: string): Promise<void> {
  await env.PAIRS.delete(`pair_token:${token}`);
}

export async function getPair(env: Env, pair_id: string): Promise<PairRecord | null> {
  return await env.PAIRS.get(`pair:${pair_id}`, "json");
}

export async function putPair(env: Env, pair_id: string, rec: PairRecord): Promise<void> {
  await env.PAIRS.put(`pair:${pair_id}`, JSON.stringify(rec));
}

export async function isRevoked(env: Env, pair_id: string): Promise<boolean> {
  return (await env.PAIRS.get(`revoked:${pair_id}`)) !== null;
}

export async function markRevoked(env: Env, pair_id: string, reason: string): Promise<void> {
  await env.PAIRS.put(
    `revoked:${pair_id}`,
    JSON.stringify({ reason, since_ts: Date.now() }),
    { expirationTtl: 60 * 60 * 24 * 90 },
  );
}

export async function isApnsDead(env: Env, pair_id: string): Promise<boolean> {
  return (await env.PAIRS.get(`apns_dead:${pair_id}`)) !== null;
}

export async function markApnsDead(env: Env, pair_id: string, reason: string): Promise<void> {
  await env.PAIRS.put(
    `apns_dead:${pair_id}`,
    JSON.stringify({ reason, since: Date.now() }),
    { expirationTtl: 60 * 60 * 24 * 90 },  // 90 days
  );
}

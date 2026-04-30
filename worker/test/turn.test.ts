import { SELF, fetchMock } from "cloudflare:test";
import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { mac_pub_b64, ios_pub_b64 } from "./helpers";

async function setupPair() {
  const mac_pub = mac_pub_b64();
  const ios_pub = ios_pub_b64();
  const create = await SELF.fetch("https://e/pair/create", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ mac_pubkey: mac_pub }),
  });
  const { pair_token, mac_device_secret } = await create.json();
  const claim = await SELF.fetch("https://e/pair/claim", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ pair_token, ios_pubkey: ios_pub }),
  });
  const { pair_id, ios_device_secret } = await claim.json();
  return { pair_id, mac_device_secret, ios_device_secret };
}

async function signTurnCred(pair_id: string, ts: number, secret_b64: string): Promise<string> {
  const msg = `turn-cred|${pair_id}|${ts}`;
  const key = await crypto.subtle.importKey(
    "raw",
    Uint8Array.from(atob(secret_b64), c => c.charCodeAt(0)),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sigBytes = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(msg));
  return btoa(String.fromCharCode(...new Uint8Array(sigBytes)));
}

describe("POST /turn/cred", () => {
  beforeEach(() => { fetchMock.activate(); fetchMock.disableNetConnect(); });
  afterEach(() => { fetchMock.assertNoPendingInterceptors(); fetchMock.deactivate(); });

  it("returns ice_servers when properly authenticated", async () => {
    const { pair_id, mac_device_secret } = await setupPair();
    const ts = Date.now();
    const sig = await signTurnCred(pair_id, ts, mac_device_secret);

    fetchMock
      .get("https://rtc.live.cloudflare.com")
      .intercept({ path: /\/v1\/turn\/keys\/.*\/credentials\/generate/, method: "POST" })
      .reply(200, {
        iceServers: [
          { urls: ["stun:stun.cloudflare.com:3478"] },
          { urls: ["turn:turn.cloudflare.com:3478?transport=udp"], username: "u", credential: "p" },
        ],
      });

    const res = await SELF.fetch("https://e/turn/cred", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts, sig }),
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(Array.isArray(body.ice_servers)).toBe(true);
    expect(typeof body.expires_at).toBe("number");
  });

  it("401 on bad signature", async () => {
    const { pair_id } = await setupPair();
    const ts = Date.now();
    const res = await SELF.fetch("https://e/turn/cred", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts, sig: btoa("bad") }),
    });
    expect(res.status).toBe(401);
    expect((await res.json()).error).toBe("bad_sig");
  });

  it("400 on ts skew", async () => {
    const { pair_id, mac_device_secret } = await setupPair();
    const ts = Date.now() - 120_000;
    const sig = await signTurnCred(pair_id, ts, mac_device_secret);
    const res = await SELF.fetch("https://e/turn/cred", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts, sig }),
    });
    expect(res.status).toBe(400);
    expect((await res.json()).error).toBe("ts_out_of_range");
  });

  it("404 on unknown pair_id", async () => {
    const res = await SELF.fetch("https://e/turn/cred", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({
        pair_id: "11111111-1111-1111-1111-111111111111",
        ts: Date.now(),
        sig: btoa("x"),
      }),
    });
    expect(res.status).toBe(404);
  });

  it("503 when Cloudflare Calls returns 5xx", async () => {
    const { pair_id, mac_device_secret } = await setupPair();
    const ts = Date.now();
    const sig = await signTurnCred(pair_id, ts, mac_device_secret);

    fetchMock
      .get("https://rtc.live.cloudflare.com")
      .intercept({ path: /\/v1\/turn\/keys\/.*\/credentials\/generate/, method: "POST" })
      .reply(500, "");

    const res = await SELF.fetch("https://e/turn/cred", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts, sig }),
    });
    expect(res.status).toBe(503);
    expect((await res.json()).error).toBe("turn_unavailable");
  });
});

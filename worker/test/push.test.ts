import { SELF, fetchMock } from "cloudflare:test";
import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { mac_pub_b64, ios_pub_b64 } from "./helpers";

async function setupPaired(): Promise<{ pair_id: string; mac_device_secret: string }> {
  const create = await SELF.fetch("https://e/pair/create", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ mac_pubkey: mac_pub_b64() }),
  });
  const { pair_token, mac_device_secret } = await create.json();
  const claim = await SELF.fetch("https://e/pair/claim", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ pair_token, ios_pubkey: ios_pub_b64(), ios_apns_token: "dummytoken_64hex" }),
  });
  const { pair_id } = await claim.json();
  return { pair_id, mac_device_secret };
}

async function signPush(pair_id: string, ts: number, secret_b64: string,
                        title: string, body: string): Promise<string> {
  const msg = `push|${pair_id}|${ts}|${title}|${body}`;
  const key = await crypto.subtle.importKey(
    "raw", Uint8Array.from(atob(secret_b64), c => c.charCodeAt(0)),
    { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
  );
  const sigBytes = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(msg));
  return btoa(String.fromCharCode(...new Uint8Array(sigBytes)));
}

describe("POST /push", () => {
  beforeEach(() => { fetchMock.activate(); fetchMock.disableNetConnect(); });
  afterEach(() => { fetchMock.assertNoPendingInterceptors(); fetchMock.deactivate(); });

  it("returns 200 pushed:true on valid signed request", async () => {
    const { pair_id, mac_device_secret } = await setupPaired();
    const ts = Date.now();
    const sig = await signPush(pair_id, ts, mac_device_secret, "build done", "exit 0");

    fetchMock.get("https://api.sandbox.push.apple.com")
      .intercept({ path: /\/3\/device\/.+/, method: "POST" })
      .reply(200, "");

    const res = await SELF.fetch("https://e/push", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts, sig, title: "build done", body: "exit 0" }),
    });
    expect(res.status).toBe(200);
    expect((await res.json()).pushed).toBe(true);
  });

  it("401 on bad sig", async () => {
    const { pair_id } = await setupPaired();
    const res = await SELF.fetch("https://e/push", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts: Date.now(), sig: btoa("bad"), title: "t", body: "b" }),
    });
    expect(res.status).toBe(401);
  });

  it("404 on unknown pair", async () => {
    const res = await SELF.fetch("https://e/push", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({
        pair_id: "11111111-1111-1111-1111-111111111111",
        ts: Date.now(), sig: btoa("x"), title: "t", body: "b",
      }),
    });
    expect(res.status).toBe(404);
  });

  it("400 on ts skew", async () => {
    const { pair_id, mac_device_secret } = await setupPaired();
    const ts = Date.now() - 120_000;
    const sig = await signPush(pair_id, ts, mac_device_secret, "t", "b");
    const res = await SELF.fetch("https://e/push", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts, sig, title: "t", body: "b" }),
    });
    expect(res.status).toBe(400);
  });

  it("410 marks dead and second call short-circuits", async () => {
    const { pair_id, mac_device_secret } = await setupPaired();
    const ts1 = Date.now();
    const sig1 = await signPush(pair_id, ts1, mac_device_secret, "t", "b");

    fetchMock.get("https://api.sandbox.push.apple.com")
      .intercept({ path: /\/3\/device\/.+/, method: "POST" })
      .reply(410, "");

    const r1 = await SELF.fetch("https://e/push", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts: ts1, sig: sig1, title: "t", body: "b" }),
    });
    expect(r1.status).toBe(410);

    // 第二次：apns_dead 已写，应直接 410，不再调 APNs
    const ts2 = Date.now();
    const sig2 = await signPush(pair_id, ts2, mac_device_secret, "t", "b");
    const r2 = await SELF.fetch("https://e/push", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts: ts2, sig: sig2, title: "t", body: "b" }),
    });
    expect(r2.status).toBe(410);
  });
});

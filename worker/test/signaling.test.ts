import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { mac_pub_b64, ios_pub_b64 } from "./helpers";

describe("WS /signal/:id (post-pair)", () => {
  it("relays JSON frames between mac and ios after both authenticate", async () => {
    // 1) create+claim 建立 pair
    const mac_pub = mac_pub_b64(), ios_pub = ios_pub_b64();
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

    // 2) 两端各 dial WS：构造 query 含 device/pair_id/ts/nonce/sig
    const macWs = await dialAuthedWS(pair_id, "mac", mac_device_secret);
    const iosWs = await dialAuthedWS(pair_id, "ios", ios_device_secret);

    // 3) mac 发一条，ios 收到原样
    macWs.send(JSON.stringify({ type: "ping", x: 42 }));
    const recv = await waitMessage(iosWs);
    expect(JSON.parse(recv).x).toBe(42);

    macWs.close(); iosWs.close();
    // 等 close 事件 & DO 异步处理完成，避免 vitest-pool-workers 存储帧出栈错误
    await new Promise(r => setTimeout(r, 50));
  });

  it("rejects WS with bad signature", async () => {
    // 建一个真实 pair，然后用错误的 secret 连接，触发 HMAC 校验失败 → 1008
    const create = await SELF.fetch("https://e/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub_b64() }),
    });
    const { pair_token } = await create.json();
    const claim = await SELF.fetch("https://e/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub_b64() }),
    });
    const { pair_id } = await claim.json();
    // 用随机错误 secret 签名（pair 存在但 sig 错）
    const wrongSecret = btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(32))));
    const ws = await dialAuthedWS(pair_id, "mac", wrongSecret);
    // 期望立刻被 close 1008 policy violation
    const closeFrame = await waitClose(ws);
    expect(closeFrame.code).toBe(1008);
    await new Promise(r => setTimeout(r, 50));
  });
});

describe("WS /signal/:id 边界", () => {
  it("rejects ts skew > 60s with 400", async () => {
    const create = await SELF.fetch("https://e/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub_b64() }),
    });
    const { pair_token, mac_device_secret } = await create.json();
    const claim = await SELF.fetch("https://e/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub_b64() }),
    });
    const { pair_id } = await claim.json();

    const ts = Date.now() - 120_000;
    const nonce = btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(16))));
    const msg = `ws-auth|mac|${pair_id}|${ts}|${nonce}`;
    const key = await crypto.subtle.importKey(
      "raw", Uint8Array.from(atob(mac_device_secret), c => c.charCodeAt(0)),
      { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
    );
    const sigBytes = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(msg));
    const sig = btoa(String.fromCharCode(...new Uint8Array(sigBytes)));
    const url = `https://e/signal/${pair_id}?device=mac&ts=${ts}&nonce=${encodeURIComponent(nonce)}&sig=${encodeURIComponent(sig)}`;
    const res = await SELF.fetch(url, { headers: { Upgrade: "websocket" } });
    expect(res.status).toBe(400);
  });

  it("rejects revoked pair with 404", async () => {
    const create = await SELF.fetch("https://e/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub_b64() }),
    });
    const { pair_token, mac_device_secret } = await create.json();
    const claim = await SELF.fetch("https://e/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub_b64() }),
    });
    const { pair_id } = await claim.json();

    // revoke the pair
    const revokeTs = Date.now();
    const revokeMsg = `revoke|${pair_id}|${revokeTs}`;
    const revokeKey = await crypto.subtle.importKey(
      "raw", Uint8Array.from(atob(mac_device_secret), c => c.charCodeAt(0)),
      { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
    );
    const revokeSigBytes = await crypto.subtle.sign("HMAC", revokeKey, new TextEncoder().encode(revokeMsg));
    const revokeSig = btoa(String.fromCharCode(...new Uint8Array(revokeSigBytes)));
    await SELF.fetch("https://e/pair/revoke", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_id, ts: revokeTs, sig: revokeSig }),
    });

    // dial WS on revoked pair — getPair returns null → 404
    const ts = Date.now();
    const nonce = btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(16))));
    const msg = `ws-auth|mac|${pair_id}|${ts}|${nonce}`;
    const key = await crypto.subtle.importKey(
      "raw", Uint8Array.from(atob(mac_device_secret), c => c.charCodeAt(0)),
      { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
    );
    const sigBytes = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(msg));
    const sig = btoa(String.fromCharCode(...new Uint8Array(sigBytes)));
    const url = `https://e/signal/${pair_id}?device=mac&ts=${ts}&nonce=${encodeURIComponent(nonce)}&sig=${encodeURIComponent(sig)}`;
    const res = await SELF.fetch(url, { headers: { Upgrade: "websocket" } });
    expect(res.status).toBe(404);
  });
});

// Helper：用 device_secret HMAC-SHA256 计算 sig，dial WS
async function dialAuthedWS(pair_id: string, device: "mac" | "ios", secret_b64: string): Promise<WebSocket> {
  const ts = Date.now();
  const nonce = btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(16))));
  const msg = `ws-auth|${device}|${pair_id}|${ts}|${nonce}`;
  const key = await crypto.subtle.importKey(
    "raw", Uint8Array.from(atob(secret_b64), c => c.charCodeAt(0)),
    { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
  );
  const sigBytes = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(msg));
  const sig = btoa(String.fromCharCode(...new Uint8Array(sigBytes)));
  const url = `https://e/signal/${pair_id}?device=${device}&ts=${ts}&nonce=${encodeURIComponent(nonce)}&sig=${encodeURIComponent(sig)}`;
  const res = await SELF.fetch(url, { headers: { Upgrade: "websocket" } });
  if (!res.webSocket) throw new Error("no websocket");
  res.webSocket.accept();
  return res.webSocket as unknown as WebSocket;
}

function waitMessage(ws: WebSocket): Promise<string> {
  return new Promise((resolve) => ws.addEventListener("message", (e: any) => resolve(e.data), { once: true }));
}

function waitClose(ws: WebSocket): Promise<{ code: number }> {
  return new Promise((resolve) => ws.addEventListener("close", (e: any) => resolve({ code: e.code }), { once: true }));
}

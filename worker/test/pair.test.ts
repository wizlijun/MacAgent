import { SELF, env } from "cloudflare:test";
import { describe, expect, it, beforeEach } from "vitest";
import { mac_pub_b64, ios_pub_b64 } from "./helpers";

describe("POST /pair/create", () => {
  beforeEach(async () => {
    const list = await env.PAIRS.list();
    await Promise.all(list.keys.map((k) => env.PAIRS.delete(k.name)));
  });

  it("returns short pair_token, room_id, mac_device_secret", async () => {
    const mac_pub = mac_pub_b64();
    const res = await SELF.fetch("https://example.com/pair/create", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub }),
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(typeof body.pair_token).toBe("string");
    expect(body.pair_token).toMatch(/^[A-HJ-NP-Z2-9]{6}$/);
    expect(typeof body.room_id).toBe("string");
    expect(body.room_id).toMatch(/^[a-f0-9-]{36}$/);
    expect(typeof body.mac_device_secret).toBe("string");

    const stored = await env.PAIRS.get(`pair_token:${body.pair_token}`, "json");
    expect(stored).toMatchObject({
      mac_pubkey_b64: mac_pub,
      mac_device_secret_b64: body.mac_device_secret,
      room_id: body.room_id,
    });
  });

  it("400 on missing mac_pubkey", async () => {
    const res = await SELF.fetch("https://example.com/pair/create", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    });
    expect(res.status).toBe(400);
    expect((await res.json()).error).toBe("invalid_mac_pubkey");
  });

  it("400 on invalid mac_pubkey base64", async () => {
    const res = await SELF.fetch("https://example.com/pair/create", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: "not!base64" }),
    });
    expect(res.status).toBe(400);
    expect((await res.json()).error).toBe("invalid_mac_pubkey");
  });

  it("400 on malformed JSON body with error=invalid_json", async () => {
    const res = await SELF.fetch("https://example.com/pair/create", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "not json{",
    });
    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body.error).toBe("invalid_json");
  });
});

describe("POST /pair/claim", () => {
  it("returns pair_id, mac_pubkey, ios_device_secret on valid token", async () => {
    const mac_pub = mac_pub_b64();
    const ios_pub = ios_pub_b64();
    const create = await SELF.fetch("https://example.com/pair/create", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub }),
    });
    const { pair_token } = await create.json();

    const res = await SELF.fetch("https://example.com/pair/claim", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub }),
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.pair_id).toMatch(/^[a-f0-9-]{36}$/);
    expect(body.mac_pubkey).toBe(mac_pub);
    expect(typeof body.ios_device_secret).toBe("string");

    const tokRec = await env.PAIRS.get(`pair_token:${pair_token}`);
    expect(tokRec).toBeNull();
    const pairRec = await env.PAIRS.get(`pair:${body.pair_id}`, "json");
    expect(pairRec).toMatchObject({ mac_pubkey_b64: mac_pub, ios_pubkey_b64: ios_pub });
  });

  it("404 on unknown pair_token", async () => {
    const res = await SELF.fetch("https://example.com/pair/claim", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token: "ZZZZZZ", ios_pubkey: ios_pub_b64() }),
    });
    expect(res.status).toBe(404);
    expect((await res.json()).error).toBe("unknown_or_expired_token");
  });

  it("400 on invalid ios_pubkey", async () => {
    const create = await SELF.fetch("https://example.com/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub_b64() }),
    });
    const { pair_token } = await create.json();
    const res = await SELF.fetch("https://example.com/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: "not!base64" }),
    });
    expect(res.status).toBe(400);
    expect((await res.json()).error).toBe("invalid_ios_pubkey");
  });

  it("404 on second claim with already-used token", async () => {
    const mac_pub = mac_pub_b64();
    const ios_pub = ios_pub_b64();
    const create = await SELF.fetch("https://example.com/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub }),
    });
    const { pair_token } = await create.json();

    // 第一次 claim：成功
    const first = await SELF.fetch("https://example.com/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub }),
    });
    expect(first.status).toBe(200);

    // 第二次 claim：token 已被消费，应 404
    const second = await SELF.fetch("https://example.com/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub }),
    });
    expect(second.status).toBe(404);
    expect((await second.json()).error).toBe("unknown_or_expired_token");
  });
});

describe("GET /pair/event/:room_id", () => {
  it("returns event after iOS claims", async () => {
    const mac_pub = mac_pub_b64();
    const ios_pub = ios_pub_b64();
    const create = await SELF.fetch("https://example.com/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub }),
    });
    const { pair_token, room_id } = await create.json();
    await SELF.fetch("https://example.com/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub }),
    });

    const res = await SELF.fetch(`https://example.com/pair/event/${room_id}`);
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.peer_joined).toBe(true);
    expect(body.ios_pubkey_b64).toBe(ios_pub);
    expect(body.pair_id).toMatch(/^[a-f0-9-]{36}$/);
  });

  it("returns 404 before iOS claims", async () => {
    const create = await SELF.fetch("https://example.com/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub_b64() }),
    });
    const { room_id } = await create.json();
    const res = await SELF.fetch(`https://example.com/pair/event/${room_id}`);
    expect(res.status).toBe(404);
  });

  it("400 on invalid room_id format", async () => {
    const res = await SELF.fetch("https://example.com/pair/event/not-a-uuid");
    expect(res.status).toBe(400);
  });
});

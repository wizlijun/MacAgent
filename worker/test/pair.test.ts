import { SELF, env } from "cloudflare:test";
import { describe, expect, it, beforeEach } from "vitest";
import { mac_pub_b64 } from "./helpers";

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
    expect(body.pair_token).toMatch(/^[A-Z2-9]{6}$/);
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
  });

  it("400 on invalid mac_pubkey base64", async () => {
    const res = await SELF.fetch("https://example.com/pair/create", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: "not!base64" }),
    });
    expect(res.status).toBe(400);
  });
});

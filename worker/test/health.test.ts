import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

describe("/health", () => {
  it("returns 200 with body 'ok'", async () => {
    const res = await SELF.fetch("https://example.com/health");
    expect(res.status).toBe(200);
    expect(await res.text()).toBe("ok");
  });

  it("404 for unknown route", async () => {
    const res = await SELF.fetch("https://example.com/nope");
    expect(res.status).toBe(404);
  });
});

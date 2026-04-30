import type { Env } from "./env";
import { b64decode, hmacVerify } from "./crypto";
import { getPair, isRevoked } from "./kv";
import { DurableObject } from "cloudflare:workers";

export class SignalingRoom extends DurableObject {
  // 同一 pair 同时只允许一对 (mac, ios)
  private peers: Map<"mac" | "ios", WebSocket> = new Map();

  override async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const pair_id = url.pathname.split("/").pop()!;
    const device = url.searchParams.get("device") as "mac" | "ios" | null;
    const ts = parseInt(url.searchParams.get("ts") ?? "0", 10);
    const nonce = url.searchParams.get("nonce") ?? "";
    const sig_b64 = url.searchParams.get("sig") ?? "";

    if (device !== "mac" && device !== "ios") return new Response("bad device", { status: 400 });
    if (!Number.isFinite(ts) || Math.abs(Date.now() - ts) > 60_000) {
      return new Response("ts out of range", { status: 400 });
    }
    if (!nonce || !sig_b64) return new Response("missing nonce/sig", { status: 400 });

    const pair = await getPair(this.env as unknown as Env, pair_id);
    if (!pair) return new Response("unknown pair", { status: 404 });
    if (await isRevoked(this.env as unknown as Env, pair_id)) {
      return new Response("pair_revoked", { status: 401 });
    }

    const secret_b64 = device === "mac" ? pair.mac_device_secret_b64 : pair.ios_device_secret_b64;
    const ok = await hmacVerify(
      b64decode(secret_b64),
      `ws-auth|${device}|${pair_id}|${ts}|${nonce}`,
      b64decode(sig_b64),
    );
    if (!ok) {
      // policy violation → 1008 close
      const pair2 = new WebSocketPair();
      pair2[1].accept();
      pair2[1].close(1008, "bad signature");
      return new Response(null, { status: 101, webSocket: pair2[0] });
    }

    const wsPair = new WebSocketPair();
    const server = wsPair[1];
    server.accept();

    // 顶替旧的同 device 连接
    const old = this.peers.get(device);
    if (old) {
      try { old.close(1000, "replaced by newer connection"); } catch {}
    }
    this.peers.set(device, server);

    server.addEventListener("message", (evt: MessageEvent) => {
      const other = device === "mac" ? this.peers.get("ios") : this.peers.get("mac");
      if (other && other.readyState === 1 /* OPEN */) {
        try { other.send(evt.data as string | ArrayBuffer); } catch {}
      }
    });
    server.addEventListener("close", () => {
      if (this.peers.get(device) === server) this.peers.delete(device);
    });

    return new Response(null, { status: 101, webSocket: wsPair[0] });
  }
}

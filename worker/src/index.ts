import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { handlePairCreate } from "./pair";

export type { Env } from "./env";

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === "/health") {
      return new Response("ok", { status: 200 });
    }

    if (url.pathname === "/pair/create" && request.method === "POST") {
      return handlePairCreate(request, env);
    }

    return new Response("not found", { status: 404 });
  },
};

// SignalingRoom 在 M1.3 实现，这里仍是 stub 让 wrangler.toml 解析
export class SignalingRoom extends DurableObject {
  override async fetch(_request: Request): Promise<Response> {
    return new Response("not implemented", { status: 501 });
  }
}

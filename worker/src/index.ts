import { DurableObject } from "cloudflare:workers";

export interface Env {
  SIGNALING_ROOM: DurableObjectNamespace;
  PAIRS: KVNamespace;
}

export default {
  async fetch(request: Request, _env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === "/health") {
      return new Response("ok", { status: 200 });
    }

    return new Response("not found", { status: 404 });
  },
};

// SignalingRoom 在 M0 是空 stub，仅为了让 wrangler.toml 能解析。
// M1 才会实现真正的信令中继逻辑。
export class SignalingRoom extends DurableObject {
  override async fetch(_request: Request): Promise<Response> {
    return new Response("not implemented", { status: 501 });
  }
}

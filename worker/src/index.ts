import type { Env } from "./env";
import { handlePairCreate, handlePairClaim, handlePairEvent, handlePairRevoke } from "./pair";

export type { Env } from "./env";
export { SignalingRoom } from "./signaling";

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === "/health") {
      return new Response("ok", { status: 200 });
    }

    if (url.pathname === "/pair/create" && request.method === "POST") {
      return handlePairCreate(request, env);
    }

    if (url.pathname === "/pair/claim" && request.method === "POST") {
      return handlePairClaim(request, env);
    }

    if (url.pathname === "/pair/revoke" && request.method === "POST") {
      return handlePairRevoke(request, env);
    }

    if (url.pathname.startsWith("/pair/event/") && request.method === "GET") {
      const room_id = url.pathname.slice("/pair/event/".length);
      return handlePairEvent(request, env, room_id);
    }

    if (url.pathname.startsWith("/signal/")) {
      const pair_id = url.pathname.slice("/signal/".length);
      const id = env.SIGNALING_ROOM.idFromName(pair_id);
      const stub = env.SIGNALING_ROOM.get(id);
      return stub.fetch(request);
    }

    return new Response("not found", { status: 404 });
  },
};

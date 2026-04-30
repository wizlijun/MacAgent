import type { DurableObjectNamespace } from "@cloudflare/workers-types";

export interface Env {
  SIGNALING_ROOM: DurableObjectNamespace;
  PAIRS: KVNamespace;
}

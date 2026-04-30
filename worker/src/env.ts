import type { DurableObjectNamespace } from "@cloudflare/workers-types";

export interface Env {
  SIGNALING_ROOM: DurableObjectNamespace;
  PAIRS: KVNamespace;
  CF_CALLS_KEY_ID?: string;
  CF_CALLS_KEY_API_TOKEN?: string;
}

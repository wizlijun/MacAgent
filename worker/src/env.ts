import type { DurableObjectNamespace } from "@cloudflare/workers-types";

export interface Env {
  SIGNALING_ROOM: DurableObjectNamespace;
  PAIRS: KVNamespace;
  CF_CALLS_KEY_ID?: string;
  CF_CALLS_KEY_API_TOKEN?: string;
  // M4 新增
  APNS_AUTH_KEY?: string;
  APNS_KEY_ID?: string;
  APNS_TEAM_ID?: string;
  APNS_BUNDLE_ID?: string;
  APNS_ENV?: string;     // "sandbox" 或 "production"（默认 prod）
}

import type { Env } from "../src/env";

declare module "cloudflare:test" {
  interface ProvidedEnv extends Env {}
}

// 测试方便：把 Response.json() 默认返回类型从 unknown 放宽到 any。
// 仅 worker/test/* 受影响（types.d.ts 通过 tsconfig include 限定）。
declare global {
  interface Response {
    json<T = any>(): Promise<T>;
  }
}

export {};

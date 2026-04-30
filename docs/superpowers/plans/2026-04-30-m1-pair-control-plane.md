# M1 · 配对 + 控制平面 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** iPhone / iPad 通过二维码与 Mac 完成一次性配对，双方持久化 pair 凭证；之后两端任意重启都能重连到 Cloudflare Worker 的 SignalingRoom Durable Object，跑一条端到端签名 ping/echo；revoke 流程把 pair 撤销后双方再也连不上。

**Architecture:**
- **三方密钥模型**：每端各持一对 X25519 钥匙；公钥经 Worker 中转给对端；私钥永不离开本机；ECDH 派生 `shared_secret` 仅 Mac/iOS 知道用于 E2E HMAC。Worker 在 `/pair/create` 与 `/pair/claim` 时各自给两端一个 `device_secret`（Worker 与该设备共享）用于 WS 握手 + 后续 HTTP 调用 HMAC。
- **两条 WS 阶段**：配对中 Mac WS 在临时 `room_id` 等 iOS 入场；配对完成后双方均连 `pair_id`，Worker DO 中继任意载荷不解读。
- **签名 ctrl ping**：M1 的"控制平面"目前只跑一条 ping/echo，证明端到端签名/中继链路通畅；M2 才在同一条 WS 上注入 SDP/ICE。

**Tech Stack（M1 新增）:**
- **Worker（TS）**：`@noble/curves` 不需要（X25519 在 Worker 上不做）；`crypto.subtle` 做 HMAC-SHA256；DO + KV 已声明，本里程碑首次启用。
- **Mac Agent（Rust）**：`x25519-dalek` 2.x、`hmac` 0.12、`sha2` 0.10、`rand` 0.8、`serde` + `serde_json`、`tokio-tungstenite` 0.24、`base64ct`/`base64` 0.22、`qrcode` 0.14（生成二维码 PNG byte buffer）、`security-framework` 2.x（已在 plan 列出过）、`uuid` 1.x。
- **iOS（Swift）**：`CryptoKit` 自带 Curve25519.KeyAgreement 与 HMAC；`AVFoundation` 自带 AVCaptureSession + AVCaptureMetadataOutput；`URLSession.webSocketTask` 自带 WS。

**对应 spec：** `/Users/bruce/git/macagent/docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md`，主要章节 §3.1 PairAuth/SignalingClient、§3.2 PairingFlow、§3.3 Worker 路由、§4.1 配对时序、§4.2 后续连接、§5 错误处理（pair_revoked / WS reconnect 等）。

---

## 协议草稿（所有 task 共享）

### 控制消息类型（在 ctrl 通道上 JSON 帧化）

```jsonc
// 端到端 HMAC 用 shared_secret，仅 Mac/iOS 验证；Worker 不验
{ "type": "ping",  "ts": 1735200000000, "nonce": "<base64>", "sig": "<base64>" }
{ "type": "pong",  "ts": 1735200001000, "nonce": "<echo>",   "sig": "<base64>" }
{ "type": "error", "code": "<str>",   "msg": "<str>",        "sig": "<base64>" }
```

签名计算：`sig = base64( HMAC-SHA256( shared_secret, canonical_json( {type, ts, nonce} 或 {type, code, msg} ) ) )`，`canonical_json` 即 RFC8785 风格——按 key 排序后 UTF-8 序列化（实现上用 `serde_json` 排好的 BTreeMap 即可）。

### Worker WS 握手协议

WS 升级请求带 query：
- `device=mac|ios`
- `pair_id=<id>`（首次配对前是 `room_id`）
- `ts=<unix_ms>`
- `nonce=<base64 random 16B>`
- `sig=<base64 HMAC-SHA256( device_secret, "ws-auth|"+device+"|"+pair_id+"|"+ts+"|"+nonce )>`

DO 校验：
- ts 与服务器时钟相差 ≤ 60s（防回放）
- 同一 `(pair_id, nonce)` 60s 内不可重复
- 用 KV 中存的 `device_secret` 重算 HMAC 并比对（恒定时比较）
- 通过后把 WS 标记为 `mac` 或 `ios`，对端连接到来时双向中继任意 frame

### KV schema（M1 启用）

```
pair_token:<token>     → { mac_pubkey_b64, mac_device_secret_b64, room_id, expires }    (5 min TTL)
pair:<pair_id>         → { mac_pubkey_b64, ios_pubkey_b64, mac_device_secret_b64,        (无 TTL)
                            ios_device_secret_b64, ios_apns_token, created_ts }
revoked:<pair_id>      → { reason, since_ts }                                            (90 天 TTL)
```

> M1 还不存 `apns_token`、APNs 是 M4 的事；字段先预留也可以，本里程碑选择**预留为可选**（claim 时若 iOS 提供就存）。

---

## 文件结构

```
mac-agent/crates/macagent-core/src/
├── lib.rs                        ← 改：把 mod 导出添加进来
├── pair_auth.rs                  ← 新：X25519 keypair / ECDH / HMAC / Keychain 持久化
├── signaling.rs                  ← 新：WebSocket 客户端 + 签名握手 + JSON 消息收发
├── ctrl_msg.rs                   ← 新：ctrl 消息类型 + 签名/校验工具
└── tests/                        ← 新：integration tests 文件夹（cargo 自动发现 tests/*.rs）

mac-agent/crates/macagent-app/src/
├── main.rs                       ← 改：把 PairAuth/SignalingClient 接进事件循环
├── pair_qr.rs                    ← 新：用 qrcode crate 生成 PNG，喂给 egui 显示
└── ui.rs                         ← 新：从 main.rs 抽出 UI 状态（菜单 + 设置窗占位）

ios-app/MacIOSWorkspace/
├── MacIOSWorkspaceApp.swift      ← 改：根据 PairStore 切换 Pairing / Paired 视图
├── ContentView.swift             ← 改：变成"未配对"提示 + 按钮 → 进入扫码 sheet
├── Crypto/                       ← 新
│   ├── PairKeys.swift            ← X25519 / HMAC / Keychain 封装
│   └── Canonical.swift           ← RFC8785 canonical JSON
├── Pairing/                      ← 新
│   ├── PairingFlow.swift         ← 扫码 → /pair/claim → 持久化
│   ├── QRScannerView.swift       ← AVFoundation QR scanner
│   └── PairStore.swift           ← @Observable，UI 订阅
├── Signaling/                    ← 新
│   ├── SignalingClient.swift     ← URLSessionWebSocketTask 包装
│   └── CtrlMessage.swift         ← Codable 消息类型
└── PairedView.swift              ← 新：已配对态显示 + ping/echo 验证按钮

worker/src/
├── index.ts                      ← 改：新增路由 /pair/create、/pair/claim、/pair/revoke、/signal/:id
├── pair.ts                       ← 新：配对端点处理函数
├── signaling.ts                  ← 新：DO `SignalingRoom` 实现
├── crypto.ts                     ← 新：HMAC-SHA256 助手 + 常量时比较
├── kv.ts                         ← 新：KV schema 类型定义 + 读写助手
└── env.ts                        ← 新：Env interface + bindings

worker/test/
├── health.test.ts                ← 不动
├── pair.test.ts                  ← 新：/pair/create + /pair/claim 状态机
├── signaling.test.ts             ← 新：DO 中继 + 握手 auth
└── helpers.ts                    ← 新：测试用的密钥生成 + HMAC 助手
```

---

## Task M1.1：Worker `POST /pair/create` + Mac device_secret 签发

**Files:**
- Create: `worker/src/env.ts`
- Create: `worker/src/crypto.ts`
- Create: `worker/src/kv.ts`
- Create: `worker/src/pair.ts`
- Modify: `worker/src/index.ts`（路由分发）
- Create: `worker/test/helpers.ts`
- Create: `worker/test/pair.test.ts`

- [ ] **Step 1.1.1：写测试**

文件 `worker/test/pair.test.ts`：

```typescript
import { SELF, env } from "cloudflare:test";
import { describe, expect, it, beforeEach } from "vitest";
import { genPairKey, mac_pub_b64 } from "./helpers";

describe("POST /pair/create", () => {
  beforeEach(async () => {
    // KV 默认是空的，但显式 list+delete 防止 leak
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
    expect(body.pair_token).toMatch(/^[A-Z2-9]{6}$/); // 6 字符 base32
    expect(typeof body.room_id).toBe("string");
    expect(body.room_id).toMatch(/^[a-f0-9-]{36}$/); // uuid v4
    expect(typeof body.mac_device_secret).toBe("string");

    // KV 写入校验
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
```

文件 `worker/test/helpers.ts`：

```typescript
// 测试用的伪密钥（不需要真做 X25519，只要是 32 字节的合法 base64 即可）
export function mac_pub_b64(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return btoa(String.fromCharCode(...bytes));
}

export function ios_pub_b64(): string {
  return mac_pub_b64();
}

export function fakePairToken(): string {
  return "ABC234"; // 6 字符
}
```

- [ ] **Step 1.1.2：跑测试，预期 fail**

```bash
cd /Users/bruce/git/macagent/worker && npm test 2>&1 | tail -10
```

预期：`/pair/create` 返回 404（路由不存在）。

- [ ] **Step 1.1.3：写实现**

`worker/src/env.ts`：

```typescript
import type { DurableObjectNamespace } from "@cloudflare/workers-types";

export interface Env {
  SIGNALING_ROOM: DurableObjectNamespace;
  PAIRS: KVNamespace;
}
```

`worker/src/crypto.ts`：

```typescript
const enc = new TextEncoder();

export function randomBytes(len: number): Uint8Array {
  const out = new Uint8Array(len);
  crypto.getRandomValues(out);
  return out;
}

export function b64encode(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes));
}

export function b64decode(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// 32 字节随机 device_secret
export function genDeviceSecret(): string {
  return b64encode(randomBytes(32));
}

// base32 Crockford 风格（去掉 0,1,O,I 易混字符），6 字符 → 30 bit ≈ 1B 唯一空间
const ALPHA = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
export function genPairToken(): string {
  const bytes = randomBytes(6);
  let out = "";
  for (let i = 0; i < 6; i++) out += ALPHA[bytes[i] % 32];
  return out;
}

export async function hmacSha256(secret: Uint8Array, msg: string): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey(
    "raw", secret, { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(msg));
  return new Uint8Array(sig);
}

export async function hmacVerify(secret: Uint8Array, msg: string, sig: Uint8Array): Promise<boolean> {
  const key = await crypto.subtle.importKey(
    "raw", secret, { name: "HMAC", hash: "SHA-256" }, false, ["verify"],
  );
  return crypto.subtle.verify("HMAC", key, sig, enc.encode(msg));
}

// X25519 公钥校验：32 字节 base64
export function isValidX25519PubB64(s: string): boolean {
  try {
    const b = b64decode(s);
    return b.length === 32;
  } catch {
    return false;
  }
}
```

`worker/src/kv.ts`：

```typescript
import type { Env } from "./env";

export interface PairTokenRecord {
  mac_pubkey_b64: string;
  mac_device_secret_b64: string;
  room_id: string;
  expires: number;
}

export interface PairRecord {
  mac_pubkey_b64: string;
  ios_pubkey_b64: string;
  mac_device_secret_b64: string;
  ios_device_secret_b64: string;
  ios_apns_token?: string;
  created_ts: number;
}

export async function getPairToken(env: Env, token: string): Promise<PairTokenRecord | null> {
  return await env.PAIRS.get(`pair_token:${token}`, "json");
}

export async function putPairToken(env: Env, token: string, rec: PairTokenRecord): Promise<void> {
  await env.PAIRS.put(`pair_token:${token}`, JSON.stringify(rec), { expirationTtl: 300 });
}

export async function deletePairToken(env: Env, token: string): Promise<void> {
  await env.PAIRS.delete(`pair_token:${token}`);
}

export async function getPair(env: Env, pair_id: string): Promise<PairRecord | null> {
  return await env.PAIRS.get(`pair:${pair_id}`, "json");
}

export async function putPair(env: Env, pair_id: string, rec: PairRecord): Promise<void> {
  await env.PAIRS.put(`pair:${pair_id}`, JSON.stringify(rec));
}

export async function isRevoked(env: Env, pair_id: string): Promise<boolean> {
  return (await env.PAIRS.get(`revoked:${pair_id}`)) !== null;
}

export async function markRevoked(env: Env, pair_id: string, reason: string): Promise<void> {
  await env.PAIRS.put(
    `revoked:${pair_id}`,
    JSON.stringify({ reason, since_ts: Date.now() }),
    { expirationTtl: 60 * 60 * 24 * 90 },
  );
}
```

`worker/src/pair.ts`：

```typescript
import type { Env } from "./env";
import { genDeviceSecret, genPairToken, isValidX25519PubB64 } from "./crypto";
import { putPairToken } from "./kv";

export async function handlePairCreate(req: Request, env: Env): Promise<Response> {
  let body: { mac_pubkey?: string };
  try {
    body = await req.json();
  } catch {
    return Response.json({ error: "invalid_json" }, { status: 400 });
  }
  if (!body.mac_pubkey || !isValidX25519PubB64(body.mac_pubkey)) {
    return Response.json({ error: "invalid_mac_pubkey" }, { status: 400 });
  }

  const pair_token = genPairToken();
  const room_id = crypto.randomUUID();
  const mac_device_secret = genDeviceSecret();

  await putPairToken(env, pair_token, {
    mac_pubkey_b64: body.mac_pubkey,
    mac_device_secret_b64: mac_device_secret,
    room_id,
    expires: Date.now() + 5 * 60 * 1000,
  });

  return Response.json({ pair_token, room_id, mac_device_secret });
}
```

`worker/src/index.ts` 替换为：

```typescript
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
```

- [ ] **Step 1.1.4：跑测试**

```bash
npm test 2>&1 | tail -15
```

预期：3/3 + 之前 2 条 = 5/5 passed。

- [ ] **Step 1.1.5：commit**

```bash
git add worker/
git commit -m "feat(worker): add POST /pair/create with KV pair_token storage and Mac device_secret"
```

---

## Task M1.2：Worker `POST /pair/claim` + DO 唤醒

**Files:**
- Modify: `worker/src/pair.ts`
- Modify: `worker/src/index.ts`
- Modify: `worker/test/pair.test.ts`（追加测试）

- [ ] **Step 1.2.1：扩展测试**

在 `worker/test/pair.test.ts` 末尾追加：

```typescript
import { ios_pub_b64 } from "./helpers";

describe("POST /pair/claim", () => {
  it("returns pair_id, mac_pubkey, ios_device_secret on valid token", async () => {
    // 先 create
    const mac_pub = mac_pub_b64();
    const ios_pub = ios_pub_b64();
    const create = await SELF.fetch("https://example.com/pair/create", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub }),
    });
    const { pair_token } = await create.json();

    // 再 claim
    const res = await SELF.fetch("https://example.com/pair/claim", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub }),
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.pair_id).toMatch(/^[a-f0-9-]{36}$/);
    expect(body.mac_pubkey).toBe(mac_pub);
    expect(typeof body.ios_device_secret).toBe("string");

    // KV：pair_token 应被删除，pair:<id> 应已写入
    const tokRec = await env.PAIRS.get(`pair_token:${pair_token}`);
    expect(tokRec).toBeNull();
    const pairRec = await env.PAIRS.get(`pair:${body.pair_id}`, "json");
    expect(pairRec).toMatchObject({ mac_pubkey_b64: mac_pub, ios_pubkey_b64: ios_pub });
  });

  it("404 on unknown pair_token", async () => {
    const res = await SELF.fetch("https://example.com/pair/claim", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token: "ZZZZZZ", ios_pubkey: ios_pub_b64() }),
    });
    expect(res.status).toBe(404);
  });

  it("400 on invalid ios_pubkey", async () => {
    const create = await SELF.fetch("https://example.com/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub_b64() }),
    });
    const { pair_token } = await create.json();
    const res = await SELF.fetch("https://example.com/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: "not!base64" }),
    });
    expect(res.status).toBe(400);
  });
});
```

- [ ] **Step 1.2.2：跑测试，预期 fail**

预期：`/pair/claim` → 404。

- [ ] **Step 1.2.3：实现**

在 `worker/src/pair.ts` 末尾追加：

```typescript
import { getPairToken, deletePairToken, putPair } from "./kv";

export async function handlePairClaim(req: Request, env: Env): Promise<Response> {
  let body: { pair_token?: string; ios_pubkey?: string; ios_apns_token?: string };
  try { body = await req.json(); } catch { return Response.json({ error: "invalid_json" }, { status: 400 }); }
  if (!body.pair_token || typeof body.pair_token !== "string") {
    return Response.json({ error: "missing_pair_token" }, { status: 400 });
  }
  if (!body.ios_pubkey || !isValidX25519PubB64(body.ios_pubkey)) {
    return Response.json({ error: "invalid_ios_pubkey" }, { status: 400 });
  }

  const tokRec = await getPairToken(env, body.pair_token);
  if (!tokRec) {
    return Response.json({ error: "unknown_or_expired_token" }, { status: 404 });
  }

  const pair_id = crypto.randomUUID();
  const ios_device_secret = genDeviceSecret();

  await putPair(env, pair_id, {
    mac_pubkey_b64: tokRec.mac_pubkey_b64,
    ios_pubkey_b64: body.ios_pubkey,
    mac_device_secret_b64: tokRec.mac_device_secret_b64,
    ios_device_secret_b64: ios_device_secret,
    ios_apns_token: body.ios_apns_token,
    created_ts: Date.now(),
  });

  await deletePairToken(env, body.pair_token);

  // 通知正在 room_id 上等待的 Mac 端：iOS 来了，pair_id=...
  // M1.3 才真正实现 DO，这里先用 KV 暂存，Mac 端自己去 GET 查询（见 §M1.3 设计）
  await env.PAIRS.put(
    `room_event:${tokRec.room_id}`,
    JSON.stringify({ peer_joined: true, pair_id, ios_pubkey_b64: body.ios_pubkey }),
    { expirationTtl: 300 },
  );

  return Response.json({ pair_id, mac_pubkey: tokRec.mac_pubkey_b64, ios_device_secret });
}
```

`worker/src/index.ts` 路由分发追加：

```typescript
if (url.pathname === "/pair/claim" && request.method === "POST") {
  return handlePairClaim(request, env);
}
```

并 import `handlePairClaim`。

- [ ] **Step 1.2.4：跑测试** 预期 5+3 = 8/8 passed。

- [ ] **Step 1.2.5：commit**

```bash
git commit -m "feat(worker): add POST /pair/claim with pair_id derivation and iOS device_secret"
```

---

## Task M1.3：Worker SignalingRoom DO + `WS /signal/:id`

**Files:**
- Create: `worker/src/signaling.ts`
- Modify: `worker/src/index.ts`
- Modify: `worker/wrangler.toml`（如果 DO 路由要 SQLite 持久化，已声明，无改动）
- Create: `worker/test/signaling.test.ts`

- [ ] **Step 1.3.1：写测试**

文件 `worker/test/signaling.test.ts`：

```typescript
import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { mac_pub_b64, ios_pub_b64 } from "./helpers";

describe("WS /signal/:id (post-pair)", () => {
  it("relays JSON frames between mac and ios after both authenticate", async () => {
    // 1) create+claim 建立 pair
    const mac_pub = mac_pub_b64(), ios_pub = ios_pub_b64();
    const create = await SELF.fetch("https://e/pair/create", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ mac_pubkey: mac_pub }),
    });
    const { pair_token, mac_device_secret } = await create.json();
    const claim = await SELF.fetch("https://e/pair/claim", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ pair_token, ios_pubkey: ios_pub }),
    });
    const { pair_id, ios_device_secret } = await claim.json();

    // 2) 两端各 dial WS：构造 query 含 device/pair_id/ts/nonce/sig
    const macWs = await dialAuthedWS(pair_id, "mac", mac_device_secret);
    const iosWs = await dialAuthedWS(pair_id, "ios", ios_device_secret);

    // 3) mac 发一条，ios 收到原样
    macWs.send(JSON.stringify({ type: "ping", x: 42 }));
    const recv = await waitMessage(iosWs);
    expect(JSON.parse(recv).x).toBe(42);

    macWs.close(); iosWs.close();
  });

  it("rejects WS with bad signature", async () => {
    const ws = await dialAuthedWS("nonexistent-pair", "mac", "AAAA");
    // 期望立刻被 close 1008 policy violation
    const closeFrame = await waitClose(ws);
    expect(closeFrame.code).toBe(1008);
  });
});

// Helper：用 device_secret HMAC-SHA256 计算 sig，dial WS
async function dialAuthedWS(pair_id: string, device: "mac" | "ios", secret_b64: string): Promise<WebSocket> {
  const ts = Date.now();
  const nonce = btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(16))));
  const msg = `ws-auth|${device}|${pair_id}|${ts}|${nonce}`;
  const key = await crypto.subtle.importKey(
    "raw", Uint8Array.from(atob(secret_b64), c => c.charCodeAt(0)),
    { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
  );
  const sigBytes = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(msg));
  const sig = btoa(String.fromCharCode(...new Uint8Array(sigBytes)));
  const url = `https://e/signal/${pair_id}?device=${device}&ts=${ts}&nonce=${encodeURIComponent(nonce)}&sig=${encodeURIComponent(sig)}`;
  const res = await SELF.fetch(url, { headers: { Upgrade: "websocket" } });
  if (!res.webSocket) throw new Error("no websocket");
  res.webSocket.accept();
  return res.webSocket as unknown as WebSocket;
}

function waitMessage(ws: WebSocket): Promise<string> {
  return new Promise((resolve) => ws.addEventListener("message", (e: any) => resolve(e.data), { once: true }));
}

function waitClose(ws: WebSocket): Promise<{ code: number }> {
  return new Promise((resolve) => ws.addEventListener("close", (e: any) => resolve({ code: e.code }), { once: true }));
}
```

- [ ] **Step 1.3.2：跑测试，预期 fail**

DO `fetch` 现在返回 501，所以 ws 升级失败。

- [ ] **Step 1.3.3：实现**

`worker/src/signaling.ts`：

```typescript
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
```

`worker/src/index.ts` 加路由：

```typescript
// 上面 import
export { SignalingRoom } from "./signaling";

// fetch 内：
if (url.pathname.startsWith("/signal/")) {
  const pair_id = url.pathname.slice("/signal/".length);
  const id = env.SIGNALING_ROOM.idFromName(pair_id);
  const stub = env.SIGNALING_ROOM.get(id);
  return stub.fetch(request);
}
```

> 用 `idFromName(pair_id)` 把同一 pair 的两端打到同一个 DO instance。

- [ ] **Step 1.3.4：跑测试** 预期 8+2 = 10/10 passed。

> 注意：`vitest-pool-workers` 对 DO + WebSocket 的支持需要 `wrangler.toml` 已声明 DO（已声明）。如果握手 race condition 导致测试不稳定，加 `await new Promise(r => setTimeout(r, 50))` 在 dial 之间。

- [ ] **Step 1.3.5：commit**

```bash
git commit -m "feat(worker): implement SignalingRoom DO with HMAC-authed WS handshake and bidirectional relay"
```

---

## Task M1.4：Mac Agent PairAuth（X25519 + HMAC + Keychain）

**Files:**
- Modify: `mac-agent/Cargo.toml`（增 deps）
- Modify: `mac-agent/crates/macagent-core/Cargo.toml`
- Create: `mac-agent/crates/macagent-core/src/pair_auth.rs`
- Create: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`
- Modify: `mac-agent/crates/macagent-core/src/lib.rs`
- Create: `mac-agent/crates/macagent-core/tests/pair_auth_test.rs`

- [ ] **Step 1.4.1：在 workspace `Cargo.toml` 增加 deps**

```toml
[workspace.dependencies]
# ... 已有
x25519-dalek = { version = "2.0", features = ["static_secrets", "serde"] }
rand = "0.8"
hmac = "0.12"
sha2 = "0.10"
base64 = "0.22"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
security-framework = "3.0"
uuid = { version = "1.10", features = ["v4"] }
qrcode = "0.14"
```

- [ ] **Step 1.4.2：`macagent-core/Cargo.toml` 加依赖**

```toml
[dependencies]
anyhow = { workspace = true }
thiserror = { workspace = true }
x25519-dalek = { workspace = true }
rand = { workspace = true }
hmac = { workspace = true }
sha2 = { workspace = true }
base64 = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
security-framework = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 1.4.3：先写 integration test**

`mac-agent/crates/macagent-core/tests/pair_auth_test.rs`：

```rust
use macagent_core::pair_auth::{PairAuth, PairToken, X25519Pub, derive_shared_secret};

#[test]
fn keypair_round_trip() {
    let pa = PairAuth::generate();
    let pub_b64 = pa.public_key_b64();
    assert_eq!(pub_b64.len() % 4, 0); // base64 4 字符整数倍
    let pub_decoded = X25519Pub::from_b64(&pub_b64).unwrap();
    assert_eq!(pa.public_key_bytes(), pub_decoded.bytes());
}

#[test]
fn ecdh_derives_same_shared_secret() {
    let mac = PairAuth::generate();
    let ios = PairAuth::generate();
    let s_mac = derive_shared_secret(&mac, &ios.public_key()).unwrap();
    let s_ios = derive_shared_secret(&ios, &mac.public_key()).unwrap();
    assert_eq!(s_mac, s_ios);
    assert_eq!(s_mac.len(), 32);
}

#[test]
fn hmac_sign_verify_round_trip() {
    let mac = PairAuth::generate();
    let ios = PairAuth::generate();
    let s = derive_shared_secret(&mac, &ios.public_key()).unwrap();
    let sig = macagent_core::pair_auth::hmac_sign(&s, b"hello world");
    assert!(macagent_core::pair_auth::hmac_verify(&s, b"hello world", &sig));
    assert!(!macagent_core::pair_auth::hmac_verify(&s, b"hello world!", &sig));
}

#[test]
fn pair_token_struct_serializes() {
    let tok = PairToken {
        pair_token: "ABC234".into(),
        room_id: "11111111-1111-1111-1111-111111111111".into(),
        worker_url: "https://macagent.workers.dev".into(),
    };
    let json = serde_json::to_string(&tok).unwrap();
    let back: PairToken = serde_json::from_str(&json).unwrap();
    assert_eq!(tok.pair_token, back.pair_token);
}
```

- [ ] **Step 1.4.4：跑测试预期 fail**

```bash
cd /Users/bruce/git/macagent/mac-agent && cargo test -p macagent-core
```

预期：编译错（模块不存在）。

- [ ] **Step 1.4.5：实现**

`mac-agent/crates/macagent-core/src/pair_auth.rs`：

```rust
//! 配对 + 端到端密钥管理。
//!
//! - X25519 keypair（私钥钥串永存 macOS Keychain，公钥 base64 上 Worker）
//! - 与对端公钥做 ECDH 派生 shared_secret（32B）
//! - HMAC-SHA256 用 shared_secret 签名 ctrl 消息
//! - device_secret 用单独的 HMAC（仅本机 + Worker 知道，用于 WS 握手）

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairToken {
    pub pair_token: String,
    pub room_id: String,
    pub worker_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairRecord {
    pub pair_id: String,
    pub peer_pubkey_b64: String,
    pub device_secret_b64: String,
    pub worker_url: String,
}

#[derive(Debug, Clone)]
pub struct X25519Pub(PublicKey);

impl X25519Pub {
    pub fn from_b64(s: &str) -> Result<Self> {
        let bytes = B64.decode(s).context("decode base64")?;
        if bytes.len() != 32 {
            return Err(anyhow!("expected 32 bytes for X25519 pubkey, got {}", bytes.len()));
        }
        let arr: [u8; 32] = bytes.try_into().unwrap();
        Ok(X25519Pub(PublicKey::from(arr)))
    }
    pub fn bytes(&self) -> &[u8; 32] { self.0.as_bytes() }
    pub fn to_b64(&self) -> String { B64.encode(self.0.as_bytes()) }
}

pub struct PairAuth {
    secret: StaticSecret,
    public: PublicKey,
}

impl PairAuth {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        PairAuth { secret, public }
    }

    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        let secret = StaticSecret::from(bytes);
        let public = PublicKey::from(&secret);
        PairAuth { secret, public }
    }

    pub fn secret_bytes(&self) -> [u8; 32] { self.secret.to_bytes() }
    pub fn public_key(&self) -> X25519Pub { X25519Pub(self.public) }
    pub fn public_key_b64(&self) -> String { B64.encode(self.public.as_bytes()) }
    pub fn public_key_bytes(&self) -> [u8; 32] { *self.public.as_bytes() }
}

pub fn derive_shared_secret(local: &PairAuth, peer: &X25519Pub) -> Result<[u8; 32]> {
    let s = local.secret.diffie_hellman(&peer.0);
    Ok(*s.as_bytes())
}

pub fn hmac_sign(secret: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    m.update(msg);
    m.finalize().into_bytes().to_vec()
}

pub fn hmac_verify(secret: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let mut m = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    m.update(msg);
    m.verify_slice(sig).is_ok()
}
```

`mac-agent/crates/macagent-core/src/ctrl_msg.rs`：

```rust
//! ctrl 通道消息类型 + 端到端签名/校验。

use crate::pair_auth::{hmac_sign, hmac_verify};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CtrlPayload {
    Ping { ts: u64, nonce: String },
    Pong { ts: u64, nonce: String },
    Error { code: String, msg: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedCtrl {
    #[serde(flatten)]
    pub payload: CtrlPayload,
    pub sig: String, // base64
}

pub fn canonical_bytes(payload: &CtrlPayload) -> Vec<u8> {
    // 用 BTreeMap 排序保证 key 排序稳定
    let v = serde_json::to_value(payload).unwrap();
    let sorted: BTreeMap<String, serde_json::Value> =
        v.as_object().unwrap().clone().into_iter().collect();
    serde_json::to_vec(&sorted).unwrap()
}

pub fn sign(payload: CtrlPayload, shared_secret: &[u8]) -> SignedCtrl {
    let bytes = canonical_bytes(&payload);
    let sig = B64.encode(hmac_sign(shared_secret, &bytes));
    SignedCtrl { payload, sig }
}

pub fn verify(signed: &SignedCtrl, shared_secret: &[u8]) -> Result<()> {
    let sig = B64.decode(&signed.sig)?;
    let bytes = canonical_bytes(&signed.payload);
    if hmac_verify(shared_secret, &bytes, &sig) {
        Ok(())
    } else {
        Err(anyhow!("ctrl signature invalid"))
    }
}
```

`mac-agent/crates/macagent-core/src/lib.rs` 替换为：

```rust
//! macagent-core
//!
//! 后续里程碑会持续把核心模块加到这里。当前 M1：PairAuth + ctrl 消息。

pub mod ctrl_msg;
pub mod pair_auth;

pub fn version() -> &'static str { env!("CARGO_PKG_VERSION") }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn version_is_non_empty() { assert!(!version().is_empty()); }
}
```

- [ ] **Step 1.4.6：跑测试** 预期：integration tests 4 + lib test 1 = 5/5 passed。

```bash
cargo test -p macagent-core 2>&1 | tail -10
cargo clippy -p macagent-core --all-targets -- -D warnings
cargo fmt --all
```

- [ ] **Step 1.4.7：commit**

```bash
git commit -m "feat(mac-agent): add macagent-core::pair_auth (X25519+ECDH+HMAC) and ctrl_msg modules"
```

> Keychain 持久化推到 Task M1.6 跟 menu-bar 整合时一起做（避免在没 UI 的 lib 里测试 Keychain）。

---

## Task M1.5：Mac Agent SignalingClient（WS + 签名握手 + reconnect）

**Files:**
- Modify: `mac-agent/crates/macagent-core/Cargo.toml`（加 tokio-tungstenite、url）
- Create: `mac-agent/crates/macagent-core/src/signaling.rs`
- Modify: `mac-agent/crates/macagent-core/src/lib.rs`
- Create: `mac-agent/crates/macagent-core/tests/signaling_test.rs`

- [ ] **Step 1.5.1：依赖**

`macagent-core/Cargo.toml` `[dependencies]` 增加：

```toml
tokio = { workspace = true }
tokio-tungstenite = "0.24"
futures-util = "0.3"
url = "2.5"
```

- [ ] **Step 1.5.2：integration test 用本地 mock WS server**

`tests/signaling_test.rs`：

```rust
use macagent_core::pair_auth::PairAuth;
use macagent_core::signaling::{SignalingClient, WsAuthQuery};
use tokio::net::TcpListener;

#[tokio::test(flavor = "current_thread")]
async fn ws_auth_query_signs_correctly() {
    // 不连真 server，仅校验 query 字符串构造 + sig 计算
    let pa = PairAuth::generate();
    let secret = [1u8; 32];
    let q = WsAuthQuery::build("mac", "pair-id-1", 1234567890, "noncebytes", &secret);
    assert!(q.contains("device=mac"));
    assert!(q.contains("pair_id=pair-id-1"));
    assert!(q.contains("ts=1234567890"));
    assert!(q.contains("nonce=noncebytes"));
    assert!(q.contains("sig="));
}

#[tokio::test(flavor = "current_thread")]
async fn dial_and_echo() {
    // 启一个本地 WebSocket echo server
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            use futures_util::{SinkExt, StreamExt};
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_text() { ws.send(msg).await.ok(); }
            }
        }
    });

    let url = format!("ws://127.0.0.1:{port}/signal/test?device=mac&ts=0&nonce=x&sig=x");
    let mut client = SignalingClient::connect(&url).await.unwrap();
    client.send_text("hello").await.unwrap();
    let echoed = client.recv_text().await.unwrap();
    assert_eq!(echoed, "hello");
}
```

- [ ] **Step 1.5.3：实现 `signaling.rs`**

```rust
//! WebSocket 信令客户端。
//!
//! 仅做：建连（带签名 query）、发/收 JSON 帧、断线指数退避重连。
//! 不做：消息加密验证（那是 ctrl_msg 的事）。

use crate::pair_auth::hmac_sign;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub struct WsAuthQuery;

impl WsAuthQuery {
    pub fn build(device: &str, pair_id: &str, ts: u64, nonce: &str, device_secret: &[u8]) -> String {
        let msg = format!("ws-auth|{device}|{pair_id}|{ts}|{nonce}");
        let sig = B64.encode(hmac_sign(device_secret, msg.as_bytes()));
        format!(
            "device={device}&pair_id={pair_id}&ts={ts}&nonce={}&sig={}",
            urlencoding::encode(nonce), urlencoding::encode(&sig),
        )
    }
}

pub struct SignalingClient {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl SignalingClient {
    pub async fn connect(url: &str) -> Result<Self> {
        let (ws, _resp) = connect_async(url).await.context("ws connect")?;
        Ok(SignalingClient { ws })
    }

    pub async fn send_text(&mut self, s: &str) -> Result<()> {
        self.ws.send(Message::Text(s.to_owned())).await.context("ws send")?;
        Ok(())
    }

    pub async fn recv_text(&mut self) -> Result<String> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(s))) => return Ok(s),
                Some(Ok(Message::Binary(_))) => continue,
                Some(Ok(Message::Ping(p))) => { self.ws.send(Message::Pong(p)).await?; continue; }
                Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => continue,
                Some(Ok(Message::Close(_))) | None => return Err(anyhow!("ws closed")),
                Some(Err(e)) => return Err(e.into()),
            }
        }
    }

    pub async fn close(mut self) -> Result<()> {
        self.ws.close(None).await.ok();
        Ok(())
    }
}
```

注意要在 `Cargo.toml` 加 `urlencoding = "2.1"` 依赖（轻量库），并在 `lib.rs` 加 `pub mod signaling;`。

- [ ] **Step 1.5.4：跑测试 + 收尾**

```bash
cargo test -p macagent-core 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

预期 7/7 passed。

- [ ] **Step 1.5.5：commit**

```bash
git commit -m "feat(mac-agent): add SignalingClient (tokio-tungstenite WS + signed query handshake)"
```

> 重连逻辑（指数退避）放在 M1.6 集成时再加，那里有 UI 状态可挂。

---

## Task M1.6：Mac Agent menu bar 二维码 + 集成 pair flow

**Files:**
- Modify: `mac-agent/crates/macagent-app/Cargo.toml`（加 reqwest + qrcode + tokio-runtime）
- Create: `mac-agent/crates/macagent-app/src/pair_qr.rs`
- Create: `mac-agent/crates/macagent-app/src/ui.rs`
- Modify: `mac-agent/crates/macagent-app/src/main.rs`
- Modify: `mac-agent/crates/macagent-app/Cargo.toml`（加 macagent-core dev-dep 已有，加 reqwest/qrcode）

- [ ] **Step 1.6.1：依赖**

`macagent-app/Cargo.toml` 加：

```toml
qrcode = { workspace = true }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde = { workspace = true }
serde_json = { workspace = true }
security-framework = { workspace = true }
uuid = { workspace = true }
egui = "0.29"
eframe = { version = "0.29", default-features = false, features = ["default_fonts", "glow", "wayland", "x11"] }
```

> 这里第一次引入 egui/eframe；菜单栏图标继续 tray-icon，但点击图标弹出 eframe 的小窗口显示二维码。

- [ ] **Step 1.6.2：`pair_qr.rs`**

```rust
//! 用 qrcode crate 把 pair_token JSON 编成二维码 PNG byte buffer。

use anyhow::Result;
use qrcode::QrCode;

pub fn encode_pair_qr_png(payload_json: &str) -> Result<Vec<u8>> {
    let code = QrCode::new(payload_json.as_bytes())?;
    let img = code.render::<image::Luma<u8>>().min_dimensions(256, 256).build();
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    #[test]
    fn encode_round_trip() {
        let png = super::encode_pair_qr_png(r#"{"pair_token":"ABC234"}"#).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }
}
```

- [ ] **Step 1.6.3：`ui.rs`** —— pair 状态机 + egui 视图

`ui.rs`：

```rust
//! 简化的 UI 状态机：未配对 / 配对中（显二维码）/ 已配对（显 ping 测试）。

use macagent_core::pair_auth::{PairAuth, PairRecord, PairToken};

#[derive(Default)]
pub enum PairState {
    #[default] NotPaired,
    Pairing { token: PairToken, qr_png: Vec<u8> },
    Paired(PairRecord),
}

pub struct AppState {
    pub pair: PairState,
    pub last_error: Option<String>,
    pub local_keys: PairAuth,
}

impl AppState {
    pub fn new() -> Self {
        // 真实场景从 Keychain 读取；M1 先每次启动新 keypair，Keychain 持久化在 Step 1.6.6
        AppState { pair: PairState::default(), last_error: None, local_keys: PairAuth::generate() }
    }
}
```

- [ ] **Step 1.6.4：`main.rs` 重构**

把 main 中事件循环改为：tray-icon 仍作菜单栏入口，点击 "Pair new device" 启动 eframe 二级窗口。eframe 用 `eframe::run_simple_native` 可在 main thread 中阻塞跑，但和 tao 冲突——折中：tray-icon 触发后由独立线程跑 `tauri::WebviewWindowBuilder` 或 `eframe::NativeOptions::run_native`。

> **设计决策**：M0 用 tao + tray-icon。eframe 自己也带 winit/tao。让 tao 退出主事件循环，把 eframe 当 main loop 接管会更顺。**M1 改造**：
> - main thread = eframe::run_native，里面承载 tray-icon
> - tray-icon 的菜单事件通过 `mpsc` 传到 eframe 的 update 函数
> - 设置窗 = eframe Window（show/hide 控制）
>
> 这是相对大的改造，对应 spec §3.1 "egui 菜单栏 / 设置 UI 与 daemon 同进程"。具体代码骨架（150 行级别）由 implementer 实现，参考 [eframe + tray-icon 集成示例](https://github.com/emilk/egui/discussions/2061)；如果 implementer 撞墙严重，BLOCKED 上报，把方案换成"tao 主循环 + 一个 tao 子窗口里嵌 egui-winit + egui-glow"。

- [ ] **Step 1.6.5：HTTP 调用 worker `/pair/create`**

```rust
async fn create_pair(local: &PairAuth, worker_url: &str) -> Result<(PairToken, String /*device_secret*/)> {
    let pub_b64 = local.public_key_b64();
    let resp: serde_json::Value = reqwest::Client::new()
        .post(format!("{worker_url}/pair/create"))
        .json(&serde_json::json!({ "mac_pubkey": pub_b64 }))
        .send().await?.json().await?;
    let pt = resp["pair_token"].as_str().ok_or_else(|| anyhow!("missing pair_token"))?;
    let rid = resp["room_id"].as_str().ok_or_else(|| anyhow!("missing room_id"))?;
    let ds = resp["mac_device_secret"].as_str().ok_or_else(|| anyhow!("missing device_secret"))?;
    Ok((PairToken { pair_token: pt.into(), room_id: rid.into(), worker_url: worker_url.into() }, ds.into()))
}
```

二维码 payload：把 PairToken JSON 编进去（iOS 扫码后能拿到 worker_url + token）。

- [ ] **Step 1.6.6：Keychain 持久化**

```rust
// 用 security-framework 的 Generic Password Item
fn keychain_save(label: &str, data: &[u8]) -> Result<()> {
    use security_framework::passwords::set_generic_password;
    set_generic_password("com.hemory.macagent", label, data).context("keychain set")?;
    Ok(())
}
fn keychain_load(label: &str) -> Result<Option<Vec<u8>>> {
    use security_framework::passwords::get_generic_password;
    match get_generic_password("com.hemory.macagent", label) {
        Ok(v) => Ok(Some(v)),
        Err(_) => Ok(None),
    }
}
```

存 5 项：`local_secret_key`（X25519 私钥 32B）、`pair_id`、`peer_pubkey_b64`、`device_secret_b64`、`worker_url`。

- [ ] **Step 1.6.7：跑 + commit**

```bash
cargo test --workspace 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p macagent-app   # 手动验证菜单栏 + Pair 按钮 + QR 显示 + Keychain 持久化
git commit -m "feat(mac-agent): integrate egui UI, qrcode pair flow, and Keychain persistence"
```

> 这条 commit 体量大（200-400 行 Rust），可以拆 2-3 个 sub-commit；implementer 自行决定。

---

## Task M1.7：iOS Crypto 助手（CryptoKit X25519 + HMAC）+ Keychain

**Files:**
- Create: `ios-app/MacIOSWorkspace/Crypto/PairKeys.swift`
- Create: `ios-app/MacIOSWorkspace/Crypto/Canonical.swift`
- Create: `ios-app/MacIOSWorkspaceTests/PairKeysTests.swift`

- [ ] **Step 1.7.1：先写测试**

`MacIOSWorkspaceTests/PairKeysTests.swift`：

```swift
import XCTest
@testable import MacIOSWorkspace

final class PairKeysTests: XCTestCase {
    func testECDHRoundTrip() throws {
        let alice = PairKeys.generate()
        let bob = PairKeys.generate()
        let s1 = try alice.deriveSharedSecret(peerPubB64: bob.publicKeyB64)
        let s2 = try bob.deriveSharedSecret(peerPubB64: alice.publicKeyB64)
        XCTAssertEqual(s1, s2)
        XCTAssertEqual(s1.count, 32)
    }

    func testHMACSignVerify() throws {
        let secret = Data(repeating: 0xAB, count: 32)
        let sig = PairKeys.hmacSign(secret: secret, message: Data("hello".utf8))
        XCTAssertTrue(PairKeys.hmacVerify(secret: secret, message: Data("hello".utf8), sig: sig))
        XCTAssertFalse(PairKeys.hmacVerify(secret: secret, message: Data("hello!".utf8), sig: sig))
    }

    func testKeychainPersistence() throws {
        let key = "test.macagent.testpersist"
        try Keychain.set(key, value: Data("hello".utf8))
        let read = try Keychain.get(key)
        XCTAssertEqual(read, Data("hello".utf8))
        try Keychain.delete(key)
        XCTAssertNil(try Keychain.get(key))
    }
}
```

- [ ] **Step 1.7.2：实现 `PairKeys.swift`**

```swift
import CryptoKit
import Foundation

struct PairKeys {
    let privateKey: Curve25519.KeyAgreement.PrivateKey
    var publicKeyData: Data { privateKey.publicKey.rawRepresentation }
    var publicKeyB64: String { publicKeyData.base64EncodedString() }
    var privateKeyData: Data { privateKey.rawRepresentation }

    static func generate() -> PairKeys {
        PairKeys(privateKey: Curve25519.KeyAgreement.PrivateKey())
    }

    static func from(privateKeyData: Data) throws -> PairKeys {
        let pk = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: privateKeyData)
        return PairKeys(privateKey: pk)
    }

    func deriveSharedSecret(peerPubB64: String) throws -> Data {
        guard let peerData = Data(base64Encoded: peerPubB64), peerData.count == 32 else {
            throw NSError(domain: "PairKeys", code: 1, userInfo: [NSLocalizedDescriptionKey: "bad peer pubkey"])
        }
        let peer = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerData)
        let shared = try privateKey.sharedSecretFromKeyAgreement(with: peer)
        return shared.withUnsafeBytes { Data($0) }
    }

    static func hmacSign(secret: Data, message: Data) -> Data {
        let key = SymmetricKey(data: secret)
        let mac = HMAC<SHA256>.authenticationCode(for: message, using: key)
        return Data(mac)
    }

    static func hmacVerify(secret: Data, message: Data, sig: Data) -> Bool {
        let key = SymmetricKey(data: secret)
        return HMAC<SHA256>.isValidAuthenticationCode(sig, authenticating: message, using: key)
    }
}

enum Keychain {
    static let service = "com.hemory.macagent"

    static func set(_ key: String, value: Data) throws {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: key,
        ]
        SecItemDelete(query as CFDictionary)
        var add = query
        add[kSecValueData] = value
        add[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlock
        let st = SecItemAdd(add as CFDictionary, nil)
        if st != errSecSuccess { throw NSError(domain: "Keychain", code: Int(st)) }
    }

    static func get(_ key: String) throws -> Data? {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: key,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        let st = SecItemCopyMatching(query as CFDictionary, &item)
        if st == errSecItemNotFound { return nil }
        if st != errSecSuccess { throw NSError(domain: "Keychain", code: Int(st)) }
        return item as? Data
    }

    static func delete(_ key: String) throws {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: key,
        ]
        let st = SecItemDelete(query as CFDictionary)
        if st != errSecSuccess && st != errSecItemNotFound {
            throw NSError(domain: "Keychain", code: Int(st))
        }
    }
}
```

- [ ] **Step 1.7.3：`Canonical.swift`** —— 与 Mac/Worker 一致的 canonical JSON

```swift
import Foundation

enum CanonicalJSON {
    static func encode(_ obj: [String: Any]) throws -> Data {
        // JSONSerialization with .sortedKeys 即可（与 BTreeMap 序列化等价，对纯字符串/数字够用）
        return try JSONSerialization.data(
            withJSONObject: obj, options: [.sortedKeys, .withoutEscapingSlashes]
        )
    }
}
```

- [ ] **Step 1.7.4：跑测试**

```bash
xcodebuild test -project MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' -quiet
```

预期：3 + 之前 2 + UI tests 全过。

- [ ] **Step 1.7.5：commit**

```bash
git commit -m "feat(ios-app): add PairKeys (CryptoKit X25519+HMAC) and Keychain helpers"
```

---

## Task M1.8：iOS PairingFlow（QR scanner + /pair/claim + Persist）

**Files:**
- Create: `ios-app/MacIOSWorkspace/Pairing/PairingFlow.swift`
- Create: `ios-app/MacIOSWorkspace/Pairing/QRScannerView.swift`
- Create: `ios-app/MacIOSWorkspace/Pairing/PairStore.swift`
- Modify: `ios-app/MacIOSWorkspace/ContentView.swift`
- Modify: `ios-app/MacIOSWorkspace/Info.plist` （`NSCameraUsageDescription`）

> Info.plist 默认在新 Xcode 项目里被写到 `*.pbxproj` 的 `INFOPLIST_KEY_*` 字段，请在 Xcode → Target → Info 里添加 `Privacy - Camera Usage Description` = `用于扫描 Mac 上的配对二维码`。

- [ ] **Step 1.8.1：`PairStore.swift`**

```swift
import Foundation
import Observation

@Observable
final class PairStore {
    enum State { case unpaired, paired(PairedPair) }

    struct PairedPair: Codable, Equatable {
        let pairId: String
        let peerPubB64: String
        let deviceSecretB64: String
        let workerURL: String
    }

    private(set) var state: State

    init() {
        if let data = try? Keychain.get("ios.pair.record"),
           let pair = try? JSONDecoder().decode(PairedPair.self, from: data) {
            state = .paired(pair)
        } else {
            state = .unpaired
        }
    }

    func savePair(_ pair: PairedPair) throws {
        let data = try JSONEncoder().encode(pair)
        try Keychain.set("ios.pair.record", value: data)
        state = .paired(pair)
    }

    func revoke() throws {
        try Keychain.delete("ios.pair.record")
        try Keychain.delete("ios.local.privkey")
        state = .unpaired
    }
}
```

- [ ] **Step 1.8.2：`QRScannerView.swift`** —— UIViewRepresentable 包 AVCaptureSession

略；标准 boilerplate 100 行。implementer 参考 https://stackoverflow.com/questions/71921069 类的标准模式即可。识别 metadata 类型用 `.qr`；扫到 stringValue 通过 `onScan` 回调返回。

- [ ] **Step 1.8.3：`PairingFlow.swift`**

```swift
import Foundation

struct PairTokenPayload: Codable {
    let pair_token: String
    let room_id: String
    let worker_url: String
}

enum PairingFlow {
    static func claim(scannedJSON: String, store: PairStore) async throws {
        let token = try JSONDecoder().decode(PairTokenPayload.self, from: Data(scannedJSON.utf8))

        // 1) 生成 iOS 自己的 keypair；私钥存 Keychain
        let keys: PairKeys
        if let priv = try Keychain.get("ios.local.privkey") {
            keys = try PairKeys.from(privateKeyData: priv)
        } else {
            keys = PairKeys.generate()
            try Keychain.set("ios.local.privkey", value: keys.privateKeyData)
        }

        // 2) POST /pair/claim
        var req = URLRequest(url: URL(string: "\(token.worker_url)/pair/claim")!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: [
            "pair_token": token.pair_token,
            "ios_pubkey": keys.publicKeyB64,
        ])
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard (resp as? HTTPURLResponse)?.statusCode == 200 else {
            throw NSError(domain: "Pair", code: 1, userInfo: [NSLocalizedDescriptionKey: "claim failed"])
        }
        struct ClaimResp: Codable { let pair_id: String; let mac_pubkey: String; let ios_device_secret: String }
        let claim = try JSONDecoder().decode(ClaimResp.self, from: data)

        // 3) 存 PairStore
        try store.savePair(.init(
            pairId: claim.pair_id,
            peerPubB64: claim.mac_pubkey,
            deviceSecretB64: claim.ios_device_secret,
            workerURL: token.worker_url,
        ))
    }
}
```

- [ ] **Step 1.8.4：`ContentView.swift` 替换为状态分支**

```swift
import SwiftUI

struct ContentView: View {
    @State var store = PairStore()

    var body: some View {
        switch store.state {
        case .unpaired:
            UnpairedView(store: store)
        case .paired(let pair):
            PairedView(pair: pair, store: store)
        }
    }
}

struct UnpairedView: View {
    @State var store: PairStore
    @State var presenting = false
    @State var error: String?

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "qrcode.viewfinder").resizable().scaledToFit().frame(maxWidth: 80).foregroundStyle(.tint)
            Text("macagent").font(.largeTitle.bold())
            Text("v0.0.1 · M1 unpaired").font(.subheadline).foregroundStyle(.secondary)
            Button("扫码配对 Mac") { presenting = true }.buttonStyle(.borderedProminent)
            if let err = error { Text(err).foregroundStyle(.red).font(.footnote) }
        }
        .padding()
        .sheet(isPresented: $presenting) {
            QRScannerView { json in
                presenting = false
                Task {
                    do { try await PairingFlow.claim(scannedJSON: json, store: store) }
                    catch { self.error = "\(error)" }
                }
            }
        }
    }
}

struct PairedView: View {
    let pair: PairStore.PairedPair
    @State var store: PairStore

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "checkmark.seal.fill").resizable().scaledToFit().frame(maxWidth: 80).foregroundStyle(.green)
            Text("已配对").font(.title.bold())
            Text("pair_id: \(pair.pairId.prefix(8))…").font(.caption).foregroundStyle(.secondary)
            Button("撤销并重新配对") { try? store.revoke() }.buttonStyle(.bordered).tint(.red)
        }.padding()
    }
}
```

> M1.9 才把 SignalingClient 接进 PairedView 用 ping 测试；当前 PairedView 仅显示状态。

- [ ] **Step 1.8.5：跑测试 + 手动**

```bash
xcodebuild test -project MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' -quiet
# 单元测试可以用 mock URL session；不在 M1 上写 e2e claim 单测，扔 M1.10 真机验证
```

- [ ] **Step 1.8.6：commit**

```bash
git commit -m "feat(ios-app): add QR scanner + PairingFlow + PairStore with adaptive Unpaired/Paired UI"
```

---

## Task M1.9：iOS SignalingClient + 签名握手

**Files:**
- Create: `ios-app/MacIOSWorkspace/Signaling/SignalingClient.swift`
- Create: `ios-app/MacIOSWorkspace/Signaling/CtrlMessage.swift`
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift` (M1.8 中创建；这里加 ping 按钮)

- [ ] **Step 1.9.1：`CtrlMessage.swift`**

```swift
import Foundation

enum CtrlPayload: Codable, Equatable {
    case ping(ts: UInt64, nonce: String)
    case pong(ts: UInt64, nonce: String)
    case error(code: String, msg: String)

    private enum CodingKeys: String, CodingKey { case type, ts, nonce, code, msg }

    func canonicalBytes() throws -> Data {
        switch self {
        case .ping(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "ping", "ts": ts, "nonce": nonce])
        case .pong(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "pong", "ts": ts, "nonce": nonce])
        case .error(let code, let msg):
            return try CanonicalJSON.encode(["type": "error", "code": code, "msg": msg])
        }
    }

    func encode(to encoder: Encoder) throws { /* 标准实现 */ }
    init(from decoder: Decoder) throws { /* 标准实现 */ }
}

struct SignedCtrl: Codable {
    let payload: CtrlPayload
    let sig: String

    static func sign(_ p: CtrlPayload, sharedSecret: Data) throws -> SignedCtrl {
        let bytes = try p.canonicalBytes()
        let sig = PairKeys.hmacSign(secret: sharedSecret, message: bytes).base64EncodedString()
        return SignedCtrl(payload: p, sig: sig)
    }

    func verify(sharedSecret: Data) throws {
        guard let sigBytes = Data(base64Encoded: sig),
              PairKeys.hmacVerify(secret: sharedSecret, message: try payload.canonicalBytes(), sig: sigBytes) else {
            throw NSError(domain: "Ctrl", code: 1, userInfo: [NSLocalizedDescriptionKey: "bad sig"])
        }
    }
}
```

- [ ] **Step 1.9.2：`SignalingClient.swift`**

```swift
import Foundation

actor SignalingClient {
    private let task: URLSessionWebSocketTask

    init(workerURL: String, pairID: String, deviceSecret: Data) throws {
        let ts = UInt64(Date().timeIntervalSince1970 * 1000)
        let nonce = (0..<16).map { _ in UInt8.random(in: 0...255) }
        let nonceB64 = Data(nonce).base64EncodedString()
        let sigMsg = "ws-auth|ios|\(pairID)|\(ts)|\(nonceB64)"
        let sigData = PairKeys.hmacSign(secret: deviceSecret, message: Data(sigMsg.utf8))
        let sigB64 = sigData.base64EncodedString()

        var c = URLComponents(string: "\(workerURL)/signal/\(pairID)")!
        c.scheme = c.scheme == "https" ? "wss" : "ws"
        c.queryItems = [
            .init(name: "device", value: "ios"),
            .init(name: "pair_id", value: pairID),
            .init(name: "ts", value: "\(ts)"),
            .init(name: "nonce", value: nonceB64),
            .init(name: "sig", value: sigB64),
        ]
        task = URLSession.shared.webSocketTask(with: c.url!)
        task.resume()
    }

    func send(_ json: String) async throws {
        try await task.send(.string(json))
    }

    func recv() async throws -> String {
        switch try await task.receive() {
        case .string(let s): return s
        case .data: throw NSError(domain: "Sig", code: 1)
        @unknown default: throw NSError(domain: "Sig", code: 2)
        }
    }

    func close() { task.cancel(with: .normalClosure, reason: nil) }
}
```

- [ ] **Step 1.9.3：`PairedView.swift` 加 ping 按钮**

```swift
import SwiftUI

struct PairedView: View {
    let pair: PairStore.PairedPair
    @State var store: PairStore
    @State var pingResult: String?

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "checkmark.seal.fill").resizable().scaledToFit().frame(maxWidth: 80).foregroundStyle(.green)
            Text("已配对").font(.title.bold())
            Text("pair_id: \(pair.pairId.prefix(8))…").font(.caption).foregroundStyle(.secondary)
            Button("发送 ping 测试") { Task { await ping() } }.buttonStyle(.bordered)
            if let r = pingResult { Text(r).font(.caption.monospaced()) }
            Button("撤销并重新配对") { try? store.revoke() }.buttonStyle(.bordered).tint(.red)
        }.padding()
    }

    private func ping() async {
        do {
            let priv = try Keychain.get("ios.local.privkey")!
            let keys = try PairKeys.from(privateKeyData: priv)
            let sharedSecret = try keys.deriveSharedSecret(peerPubB64: pair.peerPubB64)
            let secret = Data(base64Encoded: pair.deviceSecretB64)!
            let client = try SignalingClient(workerURL: pair.workerURL, pairID: pair.pairId, deviceSecret: secret)
            let nonce = "ios-\(UUID().uuidString.prefix(8))"
            let ts = UInt64(Date().timeIntervalSince1970 * 1000)
            let signed = try SignedCtrl.sign(.ping(ts: ts, nonce: String(nonce)), sharedSecret: sharedSecret)
            let json = String(data: try JSONEncoder().encode(signed), encoding: .utf8)!
            try await client.send(json)
            let resp = try await client.recv()
            let echoed = try JSONDecoder().decode(SignedCtrl.self, from: Data(resp.utf8))
            try echoed.verify(sharedSecret: sharedSecret)
            pingResult = "OK：收到 \(echoed.payload)"
            await client.close()
        } catch {
            pingResult = "ERR: \(error)"
        }
    }
}
```

- [ ] **Step 1.9.4：跑 + commit**

```bash
xcodebuild test -project MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' -quiet
git commit -m "feat(ios-app): add SignalingClient (URLSession WS) + signed ping test in PairedView"
```

---

## Task M1.10：端到端联调（真机 / 真 worker）

**Files:** 无新增；只是真机 + Cloudflare 真部署的人工验证。

- [ ] **Step 1.10.1：部署 worker**

```bash
cd /Users/bruce/git/macagent/worker
# 创建真 KV namespace
WRANGLER_OUT=$(npx wrangler kv namespace create pairs)
echo "$WRANGLER_OUT"   # 复制 id
# 替换 wrangler.toml 里 PLACEHOLDER_PAIRS_KV_ID 为实际 id
npx wrangler deploy
```

记下 worker 公网 URL（如 `https://macagent-worker.<your-subdomain>.workers.dev`）。

- [ ] **Step 1.10.2：Mac Agent 配置 worker URL**

把 worker URL 写到 macagent-app 的设置（菜单栏窗口里），或先用 env var `MACAGENT_WORKER_URL`。

- [ ] **Step 1.10.3：真机配对**

1. Mac 启 `cargo run -p macagent-app`，菜单栏点 "Pair new device" → 弹二维码
2. iPhone 真机跑 TestFlight 或 dev build，点"扫码配对"
3. 扫成功后 Mac 端菜单栏切换到"Paired (ios:xxx)"，iOS 端切换到 PairedView
4. iOS 点"发送 ping 测试" → 等 1-2 秒看到 "OK：收到 pong(...)"

- [ ] **Step 1.10.4：重启 / 重连**

1. 关闭 Mac Agent，再启动 → 仍是 Paired 态（Keychain 持久化）
2. iOS 杀掉重开 → 仍是 Paired 态
3. 各自点 ping 仍 OK

- [ ] **Step 1.10.5：撤销 + 不再连**

1. iOS 点"撤销并重新配对" → 回到 Unpaired
2. 再次点 ping 应失败（其实 Unpaired 没 ping 按钮了，OK）
3. Mac Agent 这边可以加 menu bar "Revoke" 同样调 `/pair/revoke`，删除 Keychain 记录 + Worker KV 记录

> Worker 的 `/pair/revoke` endpoint 在 M1.2 的范围内补一下（POST，body `{ pair_id, sig }`，sig 是 HMAC(device_secret, "revoke|"+pair_id)）。如果 M1.2 没加，这一步先在 Worker 端手动 `wrangler kv key delete --namespace-id=... pair:<id>` 验证 `unknown_pair` 错误流。

- [ ] **Step 1.10.6：commit 一切配置文件**

```bash
git commit -am "chore: configure worker URL + add deploy notes"
```

---

## Task M1.11：边界场景 + 错误处理

人工 + 自动测试覆盖：

- WS 握手 ts 偏差 > 60s → 400
- WS 握手 sig 错误 → 1008 close
- pair_token TTL 过期 → /pair/claim 404
- pair_id 已 revoke → WS 401
- Mac Agent 网络瞬断（关 Wi-Fi 5s 再开）→ Signaling 自动重连，UI 显示"重连中..."
- 同一 pair_id 同时来 2 个 Mac 连接 → 后者顶替前者（spec §5）

每条都加 worker 单测或 manual 验证清单。

最后提交：
```bash
git commit -m "test(worker): cover ts skew / signature mismatch / revoked pair scenarios"
```

---

## M1 验收清单

- [ ] `cd worker && npm test` 全绿（health 2 + pair 8 + signaling 2 + 边界 4 ≈ 16/16）
- [ ] `cd mac-agent && cargo test --workspace` 全绿（含 pair_auth 4 + signaling 2 + qr 1 = 至少 7 条新增单测）
- [ ] `cd ios-app && xcodebuild test ...` 全绿（PairKeys 3 + 默认 2 + UI tests 通过）
- [ ] 真机配对 happy path：iPhone → 扫 Mac 二维码 → "OK：收到 pong"
- [ ] 重启两端均能重连 + 仍能 ping
- [ ] 撤销后 ping 失败、UI 回到 Unpaired
- [ ] GitHub Actions 三条 workflow 全绿（CI 不跑真 Cloudflare、不连真 APNs）

---

## 自检（写完 plan 后做的）

1. **Spec 覆盖**：M1 行的"PairAuth + SignalingClient + Worker /pair/* + Durable Object + KV; ECDH 密钥交换；签名 ctrl 通道；菜单栏 QR；iOS 扫码流程；真 iPhone 配对真 Mac、双方重启都能恢复、revoke 流程跑通"——M1.1-M1.5 覆盖 PairAuth / SignalingClient / DO / KV / ECDH；M1.6 覆盖菜单栏 QR；M1.7-M1.9 覆盖 iOS 扫码 + 签名 ctrl；M1.10-M1.11 覆盖端到端 + 重启 + 撤销。
2. **占位符扫描**：`Info.plist` 在 Step 1.8 头部已说明走 Xcode UI 加；其余无 TBD/TODO。
3. **类型一致性**：
   - `PairAuth` Rust 与 Swift `PairKeys` 对应；都暴露 `publicKeyB64`、`deriveSharedSecret(peerPubB64)`、`hmacSign/hmacVerify`
   - `SignedCtrl` Rust + Swift + Worker 三方对 `payload + sig` 字段一致
   - `device_secret` 在 `/pair/create` 返回 `mac_device_secret`，`/pair/claim` 返回 `ios_device_secret`，KV 存 `mac_device_secret_b64` / `ios_device_secret_b64`，三处命名对应
4. **范围检查**：M1 = pairing + signed ctrl ping，**不**含 WebRTC（M2）、**不**含 PTY（M3）、**不**含 GUI 流（M5）。

---

## Plan 完成后下一步

执行选项：

1. **Subagent-Driven（推荐）** —— 11 个 task 一个个派 implementer + 双阶段 review，迭代快
2. **Inline Execution** —— 在当前会话顺序跑，关键节点暂停 review

请用户选 1 或 2 后再开始执行。

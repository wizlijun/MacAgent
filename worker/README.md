# macagent-worker

Cloudflare Workers 后端：配对 / WebRTC 信令 / APNs 上行 / TURN 凭证下发。

## 快速开始

```bash
npm install
npm test            # vitest 单测
npx wrangler dev    # 本地 :8787
```

## 端点

| 路由 | 状态 | 说明 |
|---|---|---|
| `GET /health` | ✅ M0 | 返回 `ok` 200 |
| `POST /pair/create` | ⏳ M1 | 创建一次性 pair token |
| `POST /pair/claim` | ⏳ M1 | iOS 提交扫码后的 claim |
| `WS /signal/:pair_id` | ⏳ M1 | SDP / ICE 信令中继（DO） |
| `POST /push` | ⏳ M4 | APNs 上行 |
| `POST /turn/cred` | ⏳ M2 | Cloudflare Calls TURN 凭证 |

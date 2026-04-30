# macagent

把 Mac 变成可从 iPhone / iPad 接管的应用级远程工作台。

详细设计：[`docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md`](docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md)

## 子项目

| 目录 | 内容 | 构建 |
|---|---|---|
| `mac-agent/` | Mac 端 Rust agent，菜单栏 UI 与核心 daemon | `cd mac-agent && cargo run -p macagent-app` |
| `ios-app/` | iOS / iPadOS 通用 SwiftUI 客户端 | `cd ios-app && open MacIOSWorkspace.xcodeproj` |
| `worker/` | Cloudflare Workers 后端（配对 / 信令 / APNs / TURN 凭证） | `cd worker && npm test` |

## 当前里程碑

M0 · 骨架（详见 [`docs/superpowers/plans/2026-04-30-m0-skeleton.md`](docs/superpowers/plans/2026-04-30-m0-skeleton.md)）。

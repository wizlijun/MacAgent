# macagent (Mac side)

Rust 单二进制，常驻 LaunchAgent，包含 PTY 会话管理、ScreenCaptureKit GUI 流送、
WebRTC 客户端、剪贴板桥接等核心模块（按里程碑陆续填充）。

## 快速开始

```bash
cargo run -p macagent-app
# 菜单栏出现灰色方块图标 → 点击 → Quit
```

## Crates

| crate | 类型 | 内容 |
|---|---|---|
| `macagent-core` | lib | 业务核心（PairAuth / SessionManager / GuiCapture / ...，按里程碑填充） |
| `macagent-app` | bin | 菜单栏入口 + 事件循环；M0 仅托盘图标，M1 替换为 egui UI |

## 测试

```bash
cargo test
```

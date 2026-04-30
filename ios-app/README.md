# MacIOSWorkspace (iOS / iPadOS)

通用 SwiftUI 客户端，iOS 26 / iPadOS 26。

## 在 Xcode 打开

```bash
open MacIOSWorkspace.xcodeproj
```

## 命令行测试

```bash
xcodebuild test \
  -project MacIOSWorkspace.xcodeproj \
  -scheme MacIOSWorkspace \
  -destination 'platform=iOS Simulator,name=iPhone 16'
```

## 范围

| 文件 | 说明 |
|---|---|
| `MacIOSWorkspace/MacIOSWorkspaceApp.swift` | `@main` App 入口 |
| `MacIOSWorkspace/ContentView.swift` | 占位欢迎页（M0 → M3 起替换为 NavigationSplitView 工作区） |
| `MacIOSWorkspaceTests/` | 默认 unit test target |

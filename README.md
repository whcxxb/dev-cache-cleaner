# 开发缓存清理

一个面向 macOS 开发环境的安全缓存扫描与清理工具。界面使用 Vue 3，磁盘扫描、进程占用判断和清理操作由 Tauri 2 的 Rust 原生层执行。

## 安全原则

- 前端只能提交固定目标 ID，不能传入任意文件路径。
- 每次清理前重新检查相关进程和已打开文件。
- 检测到占用时拒绝清理；pnpm dlx 会保留正在使用的条目。
- 清理过程不跟随符号链接，避免越过白名单目录。
- 项目构建缓存只删除可重新生成的目录，不删除源码和锁文件。
- pnpm store 只执行 `pnpm store prune`，保留仍被引用的包。

## 当前扫描目标

- 包管理器：uv、npm、npx、pnpm、Yarn、Homebrew
- 构建产物：pts-business、reimux-tools、Xcode DerivedData、Gradle
- 开发工具：Playwright、微信开发者工具、VS Code 更新缓存、Codex、Chrome、HBuilderX

目标白名单定义在 `src-tauri/src/cleaner.rs`。新增目标时需要同时明确路径、占用规则、清理方式和用户提示。

## 开发

```bash
pnpm install
pnpm tauri dev
```

仅启动前端：

```bash
pnpm dev
```

## 验证与构建

```bash
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build
```

macOS 应用产物位于 `src-tauri/target/release/bundle/macos/`。

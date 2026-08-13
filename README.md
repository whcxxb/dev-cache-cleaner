# 开发缓存清理

一个面向 macOS 开发环境的安全缓存扫描与清理工具。界面使用 Vue 3，磁盘扫描、进程占用判断和清理操作由 Tauri 2 的 Rust 原生层执行。

## 安装

当前 Release 提供适用于 Apple Silicon（M 系列芯片）的 DMG 安装包。

1. 下载 DMG 并将“开发缓存清理”拖入“应用程序”目录。
2. 首次启动如被 macOS 拦截，可在 Finder 中按住 Control 点击应用，选择“打开”并确认。
3. 若仍提示应用已损坏，请在终端执行以下命令后重新打开：

```bash
xattr -cr "/Applications/开发缓存清理.app"
```

此版本采用 ad-hoc 签名，适合开发者和熟悉 macOS 安全提示的用户。尚未使用 Apple Developer ID 签名或公证。

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
pnpm tauri build --bundles dmg
```

macOS DMG 产物位于 `src-tauri/target/release/bundle/dmg/`。

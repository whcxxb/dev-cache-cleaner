# DevTidy

DevTidy is a macOS utility for cleaning regenerable development caches, managing local developer-tool prompts, and checking tool versions. The interface is built with Vue 3, while disk scanning, prompt-file management, and system checks run in the Tauri 2 Rust layer.

[简体中文](#简体中文)

## Installation

The current release provides a DMG for Apple Silicon Macs (M-series chips).

1. Download the DMG and drag `DevTidy.app` into Applications.
2. If macOS blocks the first launch, Control-click the app in Finder, choose Open, and confirm.
3. If macOS still reports that the app is damaged, run the following command and open it again:

```bash
xattr -cr "/Applications/DevTidy.app"
```

This developer build uses ad-hoc signing. It is intended for developers and users familiar with macOS security prompts; it is not Developer ID signed or notarized.

## Safety model

- The frontend can submit only fixed target IDs, never arbitrary paths.
- Relevant processes and open files are checked again before every cleanup.
- Cleanup is refused when a target is in use; active `pnpm dlx` entries are retained.
- Cleanup does not follow symbolic links, preventing traversal beyond approved directories.
- Project build caches remove only regenerable directories, never source code or lockfiles.
- The pnpm store uses only `pnpm store prune`, retaining referenced packages.

## Current scan targets

- Package managers: uv, npm, npx, pnpm, Yarn, and Homebrew
- Build artifacts: pts-business, reimux-tools, Xcode DerivedData, and Gradle
- Developer tools: Playwright, WeChat DevTools, VS Code update cache, Codex, Chrome, and HBuilderX

The target allowlist lives in `src-tauri/src/cleaner.rs`. New targets must define their path, usage rules, cleanup method, and user-facing guidance.

## Development

```bash
pnpm install
pnpm tauri dev
```

To run only the frontend:

```bash
pnpm dev
```

## Verification and packaging

```bash
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build --bundles dmg
```

The macOS DMG is generated in `src-tauri/target/release/bundle/dmg/`.

---

# 简体中文

一个面向 macOS 开发环境的安全缓存扫描与清理工具。界面使用 Vue 3，磁盘扫描、进程占用判断和清理操作由 Tauri 2 的 Rust 原生层执行。

## 安装

当前 Release 提供适用于 Apple Silicon（M 系列芯片）的 DMG 安装包。

1. 下载 DMG 并将 `DevTidy.app` 拖入“应用程序”目录。
2. 首次启动如被 macOS 拦截，可在 Finder 中按住 Control 点击应用，选择“打开”并确认。
3. 若仍提示应用已损坏，请在终端执行以下命令后重新打开：

```bash
xattr -cr "/Applications/DevTidy.app"
```

此开发者版采用 ad-hoc 签名，适合开发者和熟悉 macOS 安全提示的用户，尚未使用 Apple Developer ID 签名或公证。

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

## 验证与打包

```bash
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build --bundles dmg
```

macOS DMG 产物位于 `src-tauri/target/release/bundle/dmg/`。

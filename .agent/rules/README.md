# 项目规则

## 启动前必读
- 每次开始任务先读取 `.agent/handoff.md`，只把它当跨会话上下文。
- `.agent/handoff.md` 的本地改动默认不纳入功能提交，除非任务明确要求更新接续状态。
- Windows 读取中文 Markdown、JSON、规则文件时必须显式使用 UTF-8。

## 代码交付
- 修改前先确认现有架构和调用链，优先沿用已有模块、命名和配置结构。
- 前端设置项要表达真实能力，禁止把“插件无感换号”和“桌面端重启换号”混成一个语义。
- 新增或变更配置字段时，必须同步完成以下全链路对账后才能打 tag：
  - `UserConfig`、`GeneralConfig`、`NetworkConfig` 的字段定义
  - 所有 `UserConfig { ... }` / `GeneralConfig { ... }` / `NetworkConfig { ... }` 完整初始化点
  - 前端设置映射、store、hook、service 与相关页面
  - 双语 `CHANGELOG.md` / `CHANGELOG.zh-CN.md`
  - 版本文件与 `npm run sync-version`
- Codex 切号入口当前约定：
  - `codex_switch_targets_enabled` 是点击切号同步运行端的总开关。
  - `antigravity_dual_switch_no_restart_enabled` 控制反重力 IDE 插件无感换号。
  - `codex_launch_on_switch` 控制桌面端 Codex App 凭证替换后的启动/重启换号。
- 插件端无感能力只能依赖 Antigravity/Codex 插件 runtime；桌面端不能伪装成真正无感。

## 验证要求
- TypeScript 改动至少跑 `npm run typecheck`。
- Rust/Tauri 改动至少跑 `cd src-tauri; cargo check`。
- 发布前优先跑 `npm run build`。若外层工具超时但 Vite 已输出 `built`，必须在交付里说明。
- 紧急 hotfix 若本地 `npm run tauri build` 过慢或受平台差异拖住，可在 `npm run build` 和 `cd src-tauri; cargo check` 通过后，直接以 GitHub Release workflow 作为最终封包验证；此时必须在交付里明确说明本地 bundle 未完成，以远端构建结果为准。
- 不允许只提交未验证的发布改动。

## CHANGELOG 要求
- 每个发布版本必须同时更新：
  - `CHANGELOG.md`
  - `CHANGELOG.zh-CN.md`
- 英文日志使用 `### Added` / `### Changed` / `### Fixed` 等 Keep a Changelog 标题。
- 中文日志使用 `### 新增` / `### 变更` / `### 修复`。
- changelog 必须在打 tag 前提交；漏写 changelog 时必须补提交并移动对应发布 tag。

## 版本号要求
- 发布 tag 必须与版本文件完全一致。
- 版本文件包括：
  - `package.json`
  - `package-lock.json`
  - `src-tauri/tauri.conf.json`
  - `src-tauri/Cargo.toml`
  - `src-tauri/Cargo.lock`
- 修改版本后运行 `npm run sync-version`，确认 Tauri/Cargo 版本同步。

## CI 与部署流程
- Release workflow 位于 `.github/workflows/release.yml`。
- 部署由推送 `v*` tag 触发，也支持 `workflow_dispatch`。
- CI 会校验 tag 与 `package.json` 版本一致；例如 `package.json` 为 `0.20.54` 时，tag 必须是 `v0.20.54`。
- 标准发布顺序：
  1. 完成功能代码。
  2. 更新双语 CHANGELOG。
  3. 升版本并运行 `npm run sync-version`。
  4. 运行 `npm run typecheck`、`cd src-tauri; cargo check`，发布前尽量运行 `npm run build`。
  5. 提交代码到 `main`。
  6. 打 tag：`git tag vX.Y.Z`。
  7. 推送：`git push origin main` 和 `git push origin vX.Y.Z`。
- GitHub Actions 日志里的 Node.js 20 弃用告警默认只作为维护提示，不单独阻断发布；只有 workflow 实际失败才需要修复后重新打 tag。
- 如果远端 `main` 多出 Homebrew cask 自动 PR 合并提交，必须先 `git fetch origin main`，再 rebase 到 `origin/main`，然后移动 tag 到 rebased 后的新提交。
- 禁止为了部署使用 `git push --force origin main`。只能在确认 tag 指向错误时对 tag 使用 `git push --force origin vX.Y.Z`。

## 发布后核验
- 核验远端分支和 tag：
  - `git ls-remote origin refs/heads/main refs/tags/vX.Y.Z`
- 若 `gh` 可用，查询 Release workflow：
  - `gh run list --workflow Release --limit 5`
- 若本机 `gh run list` 无输出，交付时必须明确说明无法本地确认 Actions 状态。

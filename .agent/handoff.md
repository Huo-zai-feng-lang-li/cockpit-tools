# 最新接续状态 (2026-05-28 17:50)

## 核心进展
- Codex 无感切号、配额刷新、开关隔离已完成拆分并进入可发布状态；后端配置模型补齐了 `UserConfig.codex_antigravity_plugin_hot_switch_enabled` 与 `codex_all_accounts_auto_refresh_minutes`，相关读写链路已对齐，最终发布 tag 为 `v0.20.60`，GitHub Release workflow 已成功完成。
- 核心实现分布在 `src-tauri/src/modules/config.rs`、`src-tauri/src/commands/system.rs`、`src/hooks/useAutoRefresh.ts`、`src/components/QuickSettingsPopover.tsx`、`src/pages/CodexAccountsPage.tsx`，前端/后端字段已按真实能力拆分，不再把 Antigravity 双通道切号和 Codex 插件无感换号混成一个开关。
- 本轮发布链路的最后一次 GitHub Actions 运行 `26566347448` 状态为 `Success`，5 个 matrix job 和后置步骤 `Rebuild merged latest.json` / `Upload SHA256SUMS` / `Publish release as latest` / `Update Homebrew Cask` 均已完成。

## 变更决策
- 保持 Codex 当前账号刷新与所有账号刷新彻底分离：当前账号走自己的刷新间隔，所有账号刷新默认 `3` 分钟且跳过当前账号，避免双重刷新污染状态。
- 维持插件热切失败真实透传，不再用泛化“未检测到端口”吞掉具体错误；Antigravity 插件无感换号与桌面端重启换号维持独立语义和独立配置项。
- 发布版本采取递增 patch 方式修复 CI 漏字段问题，避免在已经失败的 tag 上反复重放相同错误；`v0.20.58`、`v0.20.59` 已作为失败历史保留，当前可用发布点是 `v0.20.60`。

## 待办事项 (Next Steps)
- [ ] 新开会话时优先检查用户的手动验收结果：无感切号是否在 `Antigravity.exe --remote-debugging-port=9000` 场景下实际生效，当前账号和所有账号刷新是否按 1 分钟 / 3 分钟节拍运行。
- [ ] 如需继续打磨发布链路，再处理 `.github/workflows/release.yml` 里的 Node 20 弃用告警，升级 `actions/checkout` / `actions/setup-node` 到兼容 Node 24 的版本。
- [ ] 若后续出现配置新增字段，先同步 `UserConfig`、所有完整 initializer、前端设置映射、`CHANGELOG.md` / `CHANGELOG.zh-CN.md`，再考虑提交和打 tag。

## 关键上下文
- 目录: `C:\Users\Administrator\Desktop\超级文件\AI-IDE\AI\cockpit-tools`
- 主要文件: `src-tauri/src/modules/config.rs`, `src-tauri/src/commands/system.rs`, `src/hooks/useAutoRefresh.ts`
- 发布状态: `main` 与 `refs/tags/v0.20.60` 已对齐到提交 `18ad681f794a890614984af3017acdfad9d0da44`

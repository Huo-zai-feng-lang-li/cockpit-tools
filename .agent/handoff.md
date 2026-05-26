# 最新接续状态 (2026-05-26 22:17)

## 核心进展
- 已修复 GitHub Actions `Rebuild merged latest.json` 因 `darwin-aarch64` 更新资产命名匹配过窄导致失败的问题，关键改动在 `scripts/release/build_merged_latest_json.cjs`，并新增 `scripts/release/build_merged_latest_json.test.cjs` 覆盖 macOS 新旧资产命名。

## 变更决策
- Tauri v2 官方文档确认 updater 的 `latest.json` 需要 `platforms.<target>.url/signature`，macOS 更新资产为 `.app.tar.gz` 加 `.sig`；脚本继续生成合并版 `latest.json`，但不再只依赖 `_aarch64.app.tar.gz` / `_x64.app.tar.gz` 这种单一后缀。
- `findAsset` 已支持多个匹配器，并在缺失资产时输出实际可用资产清单，后续 CI 再失败时能直接看到 Release 资产名。
- macOS 匹配现在兼容旧命名和 target-qualified 命名：`_aarch64.app.tar.gz`、`_x64.app.tar.gz`、`aarch64-apple-darwin.app.tar.gz`、`x86_64-apple-darwin.app.tar.gz` 等。
- 已用 `node --test scripts/release/build_merged_latest_json.test.cjs` 验证通过 2 个测试；额外冒烟验证生成 `latest.json` 平台数为 15，`darwin-aarch64` / `darwin-x86_64` URL 指向正确的 `.app.tar.gz` 资产。

## 待办事项 (Next Steps)
- [ ] 视需要提交当前修复文件：`scripts/release/build_merged_latest_json.cjs`、`scripts/release/build_merged_latest_json.test.cjs`、`.agent/handoff.md`。
- [ ] 重新触发或重新推送新的 Release tag，验证 `Rebuild merged latest.json` 不再报 `Missing required updater asset for darwin-aarch64`。
- [ ] 若 CI 仍失败，优先查看新错误里打印的 `Available assets`，按真实资产名继续收窄匹配规则。

## 关键上下文
- 目录: `C:\Users\Administrator\Desktop\超级文件\AI-IDE\AI\cockpit-tools`
- 主要文件: `scripts/release/build_merged_latest_json.cjs`
- 主要文件: `scripts/release/build_merged_latest_json.test.cjs`
- 主要文件: `.github/workflows/release.yml`

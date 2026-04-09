---
description: 一键升级项目版本号（同步 package.json、tauri.conf.json、Cargo.toml）
---

# 版本号升级流程

用户会提供目标版本号（如 `0.20.22`），或者指定升级类型（patch/minor/major）。

## 步骤

1. **确定目标版本号**
   - 如果用户提供了明确版本号，直接使用
   - 如果用户说 "patch"，则从 `package.json` 读取当前版本，自动 +1 末位
   - 如果用户说 "minor"，则中位 +1，末位归 0
   - 如果用户说 "major"，则首位 +1，其余归 0

2. **同步更新三处版本号**
   使用 multi_replace_file_content 或 replace_file_content 工具，同时修改以下三个文件：
   - `package.json` 第 4 行：`"version": "x.y.z"`
   - `src-tauri/tauri.conf.json` 第 4 行：`"version": "x.y.z"`
   - `src-tauri/Cargo.toml` 第 3 行：`version = "x.y.z"`

3. **验证版本一致性**
// turbo
   ```powershell
   $pkg = (Get-Content package.json | ConvertFrom-Json).version; $tauri = (Get-Content src-tauri/tauri.conf.json | ConvertFrom-Json).version; $cargo = (Select-String -Path src-tauri/Cargo.toml -Pattern '^version = "(.+)"' | ForEach-Object { $_.Matches.Groups[1].Value }); Write-Host "package.json: $pkg"; Write-Host "tauri.conf.json: $tauri"; Write-Host "Cargo.toml: $cargo"; if ($pkg -eq $tauri -and $tauri -eq $cargo) { Write-Host "✅ 版本一致: $pkg" } else { Write-Host "❌ 版本不一致!" }
   ```

4. **可选：更新 CHANGELOG**
   如果用户需要，在 `CHANGELOG.md` 和 `CHANGELOG.zh-CN.md` 顶部添加新版本条目。

5. **可选：Git 提交 & Tag**
   ```powershell
   git add -A
   git commit -m "chore: bump version to x.y.z"
   git tag vx.y.z
   ```

# Codex Usage Monitor

一个本地优先的 macOS 菜单栏应用，用于只读查看当前 Codex 登录账户的额度、本机 Codex Token 用量和系统健康状态。应用不会切换 Codex 登录，也不会保存会话正文。

## 当前能力和边界

- Rust 每 2 秒采集 CPU、macOS 内存压力、根磁盘、网络吞吐、电池和运行时长；只持久化分钟/小时聚合。
- 通过本机 Codex `app-server --stdio` 只读请求 `account/read` 和 `account/rateLimits/read`，显示动态命名额度窗口、剩余百分比、重置时间及 stale/error 状态。
- 流式读取 `$CODEX_HOME/sessions`、`$CODEX_HOME/archived_sessions`（未设置时为 `~/.codex/...`）中的 JSONL，只提取 Token 数值、模型和不透明会话标识；支持增量、归档、截断恢复、幂等导入和手工归属修正。
- 支持暂停、保留期、清理历史、账户范围历史删除，以及无凭据的 CSV/JSON 导出。
- 额度、认证、连续过期、磁盘和严重内存压力通知跨重启去重，恢复后可再次通知。
- 默认简体中文，支持英文、浅色/深色/系统主题、键盘焦点、VoiceOver 名称和非颜色状态。

独立多账户 OAuth、Keychain 凭据管理及重新授权仍依赖 Issue #2/#4 的真实账户验证。当前构建只观察 Codex 已登录的真实账户，不会把只读回退伪装成托管多账户，也不会报告不存在的凭据删除成功。

## 构建与验证

需要 Node.js、npm、Rust 和 macOS 12 或更高版本：

```bash
npm ci
npm run typecheck
npm run test:run
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
npm run tauri build
```

release app 位于：

```text
src-tauri/target/release/bundle/macos/Codex Usage Monitor.app
```

Tauri 构建会对整个 bundle 做 ad-hoc 签名。可用 `codesign --verify --deep --strict --verbose=4 <app-path>` 验证。首版不含 Developer ID 签名、公证或自动更新；若 macOS 首次打开给出来源提示，请在 Finder 中右键应用并选择“打开”，由用户明确确认本地构建。

完整自动验收结果、性能数据和人工阻塞项见 [发布资格记录](docs/release-qualification.md)。

## 数据位置与导出

本地 SQLite：

```text
~/Library/Application Support/com.zzjnotes.codex-usage-monitor/monitor.sqlite3
```

同目录可能存在 SQLite 的 `-wal` 和 `-shm` 临时文件。数据库保存偏好、额度快照、Token 数值事件、不透明会话归属、系统聚合、通知状态和无路径的导入 checkpoint；不保存 OAuth 凭据、提示词、回复、命令、附件或工作路径。应用不创建本地日志文件。

CSV/JSON 导出保存到 `~/Downloads/codex-usage-YYYY-MM-DD.{csv,json}`；重名时自动增加数字后缀。导出采用允许字段白名单并将会话 ID 再次散列，但导出仍属于用户文件，卸载时不会自动删除。

## 清理、重新授权与卸载

在仪表盘“数据与隐私”中可以设置保留期、立即执行过期清理、清除全部历史或按账户删除历史。清理保留不含路径的导入 checkpoint，避免旧会话被再次导入；新产生的 Token 活动仍可继续采集。

当前版本没有独立 OAuth/Keychain 凭据，因此没有应用内重新授权或凭据删除入口。额度认证异常时只给出恢复说明；Codex 登录由 Codex 自己管理，应用不会读取、复制、刷新或覆盖 `auth.json`。未来启用托管账户后，重新授权和“仅删凭据/凭据及历史”必须继续与 Codex 当前登录隔离。

卸载步骤：

1. 从菜单栏退出 Codex Usage Monitor。
2. 在 Finder 中将 `Codex Usage Monitor.app` 移到废纸篓。
3. 如需同时删除统计，在 Finder 的“前往文件夹”中打开 `~/Library/Application Support/com.zzjnotes.codex-usage-monitor/`，确认路径后将该目录移到废纸篓。
4. 如需删除导出，在 `~/Downloads` 中单独确认并移除 `codex-usage-*.csv` 和 `codex-usage-*.json`。

删除应用本身不会自动删除数据库或导出。当前版本不会创建或删除 Keychain 项。

## 隐私与网络边界

React 只接收 typed IPC DTO，不接收 OAuth token。应用源码不包含通用 HTTP 客户端、遥测、远程日志、云同步、LAN 管理或自动更新；直接外部进程调用只有本机 Codex app-server（额度）和 `/usr/bin/pmset -g batt`（电池）。额度网络及凭据刷新由 Codex 自身承担，应用只通过 stdio 观察结果。CSP 只允许 Tauri 本地 IPC/asset origin。

真实网络端点、两个账户授权隔离、Keychain 内容以及 Notification Center 授权必须在人工 E2E 中验证；不能从 fixture 或静态扫描推定为通过。

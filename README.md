# Codex Usage Monitor

一个本地优先的 macOS 菜单栏应用。当前 S2 版本提供 Tauri 应用外壳、SQLite 基础、系统健康面板，以及暂停/恢复、中文/英文和主题偏好。

## 当前能力

- 每 2 秒在 Rust 侧采集 CPU、macOS 内存压力、根磁盘、网络吞吐、电池和运行时长，并保留一小时内存环形缓冲。
- 只向 React 层发送可展示的 IPC DTO；界面提供 loading、empty、error 和非颜色状态文本。
- 关闭仪表盘只隐藏窗口，应用继续驻留菜单栏；从菜单栏可重新打开、暂停/恢复或退出。
- 偏好和分钟/小时系统聚合保存在本机 SQLite，不保存原始逐次采样。
- 数据库不可用时进入临时内存模式，并在界面给出原因和恢复动作。
- 默认简体中文，支持英文、浅色、深色和跟随系统，并提供键盘焦点与 VoiceOver 名称。
- 通过本机 Codex app-server 只读显示当前 ChatGPT 登录账户的动态命名额度窗口、剩余百分比和重置时间；最近一次有效快照保存在 SQLite；不会读取或复制 OAuth token，也不会切换 Codex 登录。应用不会在无法确认当前登录账户时显示历史快照，避免跨账户错配。
- 通过 macOS 本地通知提醒额度 20%/10%/0%、OAuth 失效、连续刷新失败、磁盘空间不足和严重内存压力；通知状态保存在 SQLite，恢复前不会重复发送，且文案包含原因、影响和恢复动作。CPU 与网络短时波动不会触发通知。

当前额度能力仅覆盖“当前 Codex 登录账户”的只读视图。通知状态机已按账户标识隔离，可供未来所有真实托管账户复用，但独立授权和管理多个 ChatGPT 账户仍依赖 Issue #2/#4 的双真实账户兼容性验证；当前版本不会伪造多账户通知或重新授权入口。导出和完整发布验收仍属于后续产品切片。

## 开发与验证

```bash
npm install
npm run typecheck
npm run test:run
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --debug
```

生成的本机 app 位于：

```text
src-tauri/target/debug/bundle/macos/Codex Usage Monitor.app
```

## 本地数据

SQLite 数据库位于：

```text
~/Library/Application Support/com.zzjnotes.codex-usage-monitor/monitor.sqlite3
```

暂停监控会停止系统采样；暂停偏好会跨应用重启保存。删除应用不会自动删除数据库。需要完全清理时，先从菜单栏退出应用，再手动移除上述应用数据目录。当前版本不请求 OAuth 或其他账户权限。

## 隐私边界

当前版本不访问 Codex 会话、提示词、回复、命令、附件或工作路径，不发送遥测。额度刷新调用本机 Codex app-server 的 `account/read` 和 `account/rateLimits/read`；网络通信及凭据刷新仍由 Codex 自身负责，应用 UI 和 SQLite 不接收凭据。电池状态通过 macOS `pmset` 每 30 秒以内最多读取一次；其余指标通过本机 Rust/macOS API 获取。

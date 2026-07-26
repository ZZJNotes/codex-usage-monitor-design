# Codex Usage Monitor

一个本地优先的 macOS 菜单栏应用。当前 S2 版本提供 Tauri 应用外壳、SQLite 基础、系统健康面板，以及暂停/恢复、中文/英文和主题偏好。

## 当前能力

- 每 2 秒在 Rust 侧采集 CPU、macOS 内存压力、根磁盘、网络吞吐、电池和运行时长。
- 只向 React 层发送可展示的 IPC DTO；界面提供 loading、empty、error 和非颜色状态文本。
- 关闭仪表盘只隐藏窗口，应用继续驻留菜单栏；从菜单栏可重新打开、暂停/恢复或退出。
- 偏好和分钟/小时系统聚合保存在本机 SQLite，不保存原始逐次采样。
- 默认简体中文，支持英文、浅色、深色和跟随系统，并提供键盘焦点与 VoiceOver 名称。

账户额度与本机 Codex Token 统计属于后续产品切片，不在 S2 范围内。

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

当前版本不访问 Codex 会话、提示词、回复、命令、附件或工作路径，不发送遥测，也不产生应用自身的网络请求。电池状态通过 macOS `pmset` 每 30 秒以内最多读取一次；其余指标通过本机 Rust/macOS API 获取。

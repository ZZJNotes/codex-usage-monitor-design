# S10 发布资格验证记录

验证日期：2026-07-27（Asia/Shanghai）  
规格：GitHub Issue #11，父规格 #1  
基线：`d155fe8`（包含 #5-#10 自动实现；#4 真实多账户工作仍未完成）  
结论：**自动范围通过，完整发布资格暂为 conditional / blocked by human E2E。**

## 目标环境

| 项目 | 值 |
| --- | --- |
| 机器 | Apple M1 Pro，arm64，32 GiB RAM |
| 系统 | macOS 27.0 (26A5388g) |
| Node / npm | v22.22.2 / 10.9.7 |
| Rust / Cargo | 1.94.1 / 1.94.1 |
| app | 0.1.0，13 MiB，最低 macOS 12.0 |

## 自动验证结果

| 范围 | 结果 | 证据 |
| --- | --- | --- |
| Rust 全量测试 | 通过 | 75 passed；1 个真实 Codex 登录测试 ignored |
| Frontend | 通过 | 15 tests；TypeScript typecheck；Vite production build |
| Rust 格式/静态检查 | 通过 | `cargo fmt --check`；all-target/all-feature Clippy with `-D warnings` |
| Release bundle | 通过 | `npm run tauri build` 生成 release `.app` |
| ad-hoc 签名 | 通过 | bundle `Identifier=com.zzjnotes.codex-usage-monitor`，flags `adhoc,runtime`；`codesign --verify --deep --strict` valid on disk / satisfies Designated Requirement |
| Gatekeeper 本机评估 | 有条件通过 | `spctl --assess` 返回 accepted，但本机显示 `override=security disabled`，因此不能外推到其他 Mac；Developer ID/公证不在首版范围 |
| 隐私回归 | 通过 | SQLite schema、实际隔离 DB bytes、CSV/JSON、UI state、diagnostics 和不存在的 log surface 扫描；未发现凭据或禁止的会话字段 |
| Session fixture | 通过 | 仅使用仓库脱敏 active/archive JSONL；覆盖坏行、未知事件、截断、移动归档、增量、重复扫描、Token 子集语义和归属冻结 |
| 故障恢复 | 通过 | wake/network cooldown、bounded backoff、single-flight、restart stale、暂停重启、通知持久去重、认证/transport failure、数据库失败 DTO 测试通过 |
| 网络/telemetry 静态扫描 | 通过 | 无通用 HTTP client、socket、telemetry/analytics/remote logging 依赖；直接外部进程仅 Codex app-server stdio 与 `pmset`；CSP 仅本地 IPC/asset |

fixture 中的 `PRIVATE_FIXTURE_SENTINEL`、`/REDACTED/never-store` 等内容是有意放置的脱敏反例，用于证明输出不会保存正文或路径；它们不是用户真实会话。

## 本机隔离运行

release binary 在临时 `CFFIXED_USER_HOME`/`HOME`、空 `CODEX_HOME` 下冷启动，并将 `CODEX_USAGE_MONITOR_CODEX_PATH` 指向 `/usr/bin/false`。因此本次运行未读取真实 `~/.codex`、`auth.json`、Keychain 或会话正文，也没有触发 Codex 网络访问。

- 进程持续运行 21 秒，状态正常。
- 隔离 SQLite 创建完成：746 ms。该值是 app setup/storage-ready 代理，不等同于可视 dashboard 首帧。
- 10 秒 warm-up 后 5 次 idle CPU：0.4%、0.4%、0.2%、0.2%、0.2%，均低于 1% 目标。
- 同期 RSS：123840、124272、124480、124640、124720 KiB；峰值约 121.8 MiB，低于 150 MiB 目标。
- `lsof -a -p <pid> -i` 无输出；隔离进程没有打开 TCP/UDP socket。
- 隔离数据目录只出现 SQLite、WAL 和 SHM；实际 DB byte scan 未命中 token、prompt、reply、command、attachment、work path 或 `/Users/` 标记。

由于主窗口按产品设计初始隐藏、仓库没有不依赖 UI/Accessibility 授权的“dashboard painted” readiness 信号，本次不把 746 ms 伪报为 dashboard cold start。可视 dashboard 2 秒目标和菜单栏更新 1 秒目标保留给人工本机验收。

## 查询性能

服务边界性能 probe 生成 10,000 条脱敏 Token 事件，导入到内存 SQLite 后通过真实 `TokenUsageService::query` 测量：

| 查询 | 数据量 | 结果 | 目标 |
| --- | ---: | ---: | ---: |
| 全历史聚合 | 10,000 events / 1 session / 1 model | 22 ms | 约 500 ms |
| model + session 筛选 | 10,000 events / 1 session / 1 model | 23 ms | 约 500 ms |

测试 `common_history_queries_meet_the_release_target_on_ten_thousand_sanitized_events` 固化了数据量、正确总数和 500 ms 门槛。该结果覆盖常见查询，不代表极端多 session/model 基数。

## 产物

```text
src-tauri/target/release/bundle/macos/Codex Usage Monitor.app
```

主可执行文件 SHA-256：

```text
4c951e57cfeda154b6431edbda9042e539f2e44f3fb48c159e8152a707051b4b
```

构建产物未提交 Git；应从本提交在目标 Mac 重新执行 `npm ci && npm run tauri build`。Tauri 配置固定 `signingIdentity: "-"`，避免只签 Mach-O 而未签完整 bundle。

数据位置、导出、清理、重新授权边界和卸载步骤见根目录 README。

## 人工阻塞项（未执行、未伪造通过）

以下项目需要真实账户、系统授权或真实用户数据，自动验收明确跳过：

1. 两个真实 ChatGPT 账户的独立 OAuth、Keychain 保存/刷新/删除、别名/置顶和重新授权；验证不会改变 Codex 当前登录或写入 `auth.json`。
2. 当前未登录 Codex 的第二账户额度刷新，以及两个账户的窗口隔离、stale/backoff 和删除语义。
3. 真实 active/archive Codex 会话首次导入、实时追加延迟、无重复总数、账户切换后的归属冻结和人工修正。
4. 真实 macOS 睡眠/唤醒、断网/恢复下的请求节流与恢复行为，以及真实网络抓包确认仅允许的 OpenAI 流量。
5. Notification Center 首次授权、实际投递、拒绝/恢复和跨重启去重 E2E。
6. 可视 dashboard cold paint < 2 秒、菜单栏值更新 < 1 秒、键盘和 VoiceOver 人工验收。
7. 在真实数据上扫描 SQLite、导出及本地诊断；执行时必须由用户授权，且不得把原始会话正文或 Keychain secret 收集进报告。

在这些项目完成前，不应关闭 #11 或宣称首版已取得完整发布资格。

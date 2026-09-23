# xTermius MCP V1

本文档是 xTermius MCP V1 的架构、安全边界、客户端配置和验收记录。MCP 默认关闭；打开后，所有配对、连接授权和命令审批仍由 xTermius 用户控制。

## 架构

```text
Claude Code / OpenCode
        │ stdio MCP (JSON-RPC)
        ▼
xtermius-mcp-bridge
        │ Unix socket, 0600, private directory 0700
        ▼
xTermius Rust core
  connection registry → grants → approval/task engine → audit ledger
        │
        ▼
user-created, authenticated OpenSSH master connection
```

- Bridge 只负责 stdio MCP 与本机 IPC 的协议转换，不实现授权，也不接收命令行 token。
- 授权、代次校验、审批、取消和审计全部在 app 内完成；不会启动远端守护进程，也不会为 MCP 重新输入密码、私钥或 MFA。
- 用户可在设置页生成一次性 pairing token。token 只以 SHA-256 哈希写入本机 `mcp.db`；pairing identity 会跨 app restart 保留。
- operation grant 绑定 `(client_id, connection_id, generation)`，app restart 时 `mcp_grants` 全部清除，不会跨 restart 自动恢复。
- `mcp.db` 与 WebDAV 同步的 hosts 数据库分离，pairing token、grant 和 audit ledger 不离开本机。
- app 启动时必须成功打开 pairing store 和 audit ledger；任一 SQLite store 打开或初始化失败，MCP service 不启动，不能降级到 memory-only MCP。

## Bridge 与五个 MCP 工具

设置页的 `mcp_bridge_info` 根据当前 app `current_exe` 的 sibling 解析 bridge 路径，不使用 `PATH` 查找，也不返回 socket 路径或 secret。返回环境标记 `development` 或 `packaged`。

Bridge 暴露且仅暴露以下五个工具：

| 工具 | 行为 |
| --- | --- |
| `list_connections` | 列出当前连接，以及该配对 client 被授予的 observe/execute 权限。 |
| `read_connection_output` | 从 grant 起始序号之后读取观察缓存；游标过期会显式报告 gap。 |
| `run_command` | 提交不可变的命令、`login` working directory 和 timeout，返回待审批 task。 |
| `read_task` | 读取 task 状态，以及独立的 stdout/stderr byte cursor；没有合并顺序承诺。 |
| `cancel_task` | 请求取消 task；取消是异步的，必须继续用 `read_task` 确认结果。 |

MCP 工具不会返回或写入 pairing token、SSH 密码或私钥；获得 Execute/Observe 授权的 client 仍可通过 `read_task` 读取自己的 stdout/stderr。审计记录只保存请求与生命周期元数据，不保存 stdout/stderr。

设置页通过受信任的 Tauri command `mcp_recent_task_audit(limit)` 读取 newest-first 审计元数据，`limit` 只能是 1–100；该 command 同样不返回 output。

## 默认关闭、分权与风险提示

- MCP 默认关闭。关闭时不接受 agent IPC；关闭操作会撤销 operation grants、取消 task 并清空观察缓存。
- Observe 与 Execute 独立授权：Observe 只读取该连接 grant 起始点之后的终端输出；Execute 允许提交命令，但每一条命令仍须人工审批。
- Observe 会把终端数据发送给外部模型；xTermius 无法保证检测出 secret。不要对含有不能外发内容的会话开启 Observe。
- Execute 使用现有 SSH account 的权限，可能是 root 权限。审批不是权限降级，也不是 shell sandbox。
- 审批面板和审计记录显示 `working_directory`、timeout 和 approval expiry。V1 working directory 固定为 SSH login directory，省略时默认 `login`；approval window 为 5 分钟；timeout 范围为 1–1800 秒，默认 300 秒。

## 连接、授权与任务生命周期

连接只有通过受管 OpenSSH master 的 `-O check` 后才进入 `ready`。密码、指纹或 MFA 提示期间保持 `connecting`，设置页不会允许为非 `ready` 连接勾选权限。

grant 是具体连接代次的授权。重连、关闭、退出、撤销或 client transport EOF 都会使旧 grant 失效；需要用户重新为新连接代次授权。

命令参数在 `run_command` 时捕获并写入审计，approval 不接受另一份命令或参数。task 状态包括：

- `pending_approval` → `queued` → `dispatching` → `running`；
- 取消请求先进入 `cancel_requested`，只有确认本地 SSH channel 已结束才进入 `cancelled`；
- 正常退出为 `completed` 或 `failed`，超时为 `timed_out`；
- master/连接在结果确认前断开为 `unknown_after_disconnect`，绝不自动重放；
- 撤销、过期、拒绝或代次不匹配的待审批 task 为 `rejected`。

`request_id` 在 `(client_id, request_id)` 范围内幂等。相同 request 会返回原 task，不会重复执行；不同参数会冲突。跨 app restart 的旧 request 会被拒绝，不会从旧审计记录自动重放。

## V1 限制与安全上限

所有 buffer、cursor 和读响应的大小均按 UTF-8 byte 计数，不按 Unicode 字符数计数。

| 项目 | 上限或规则 |
| --- | --- |
| paired clients | 最多 32 个 |
| IPC active connections | 最多 32 个；超过后 fail-closed，不额外创建 worker |
| IPC rate | 每 client 每秒 60 个请求；全局每秒 120 个请求 |
| IPC frame | 单行最多 64 KiB；超限帧会被丢弃并继续消费到 delimiter |
| Observe buffer | 每连接 1 MiB UTF-8 bytes；全局 8 MiB UTF-8 bytes；慢 client 丢失历史时报告 gap |
| Observe read | 单次最多 32 KiB UTF-8 bytes；`max_chars` 仍受同一 32 KiB byte cap 约束 |
| Task output retention | 每 task 的 stdout+stderr 共 1 MiB UTF-8 bytes |
| Task read | 单次 `read_task` 的 stdout+stderr 共 32 KiB UTF-8 bytes，stdout/stderr 使用独立 byte cursor |
| Pending approvals | 每 client 8 个，全局 32 个 |
| Retained tasks | 最多 256 个 task；active task 不会被 cap sweep 删除 |
| Approval expiry | 5 分钟；过期转为 `rejected` |
| Command timeout | 1–1800 秒，默认 300 秒；working directory 固定为 `login` |
| Audit retention | 保留最近 30 天，最多 10,000 条；active audit row 不因 cap 删除 |

## 本机 IPC、EOF、session lock 与多实例

- socket 位于 app config 下的 `mcp/bridge.sock`；父目录 mode 0700，socket mode 0600。
- macOS accept 时使用 `getpeereid`，要求 peer UID 等于 app effective UID；其他 Unix 平台依赖 private directory 与 socket mode 的本机边界。
- 同一路径已有可连接 listener 时，第二个 app 返回 `AlreadyRunning`，不会破坏第一个 listener。只有连接明确失败的 stale socket 才会清除；普通文件占用路径会直接报错。
- bridge stdio EOF 会撤销该 transport 绑定 client 的 operation grants，取消其 task，并清理不再被其他 observer 使用的 buffer；pairing identity 保留，下一次 bridge 仍须重新授权。
- macOS login session resign/lock 会撤销 grants、清空 observation buffers、取消 pending/queued task，并对 dispatching/running task 发出 best-effort cancel；pairing identity 和 enabled preference 保留，恢复后需要重新授权。
- 这不是对同一 OS account 下恶意本机进程的防护；socket 权限和 token 只构成本机配对边界。

## 受管 OpenSSH 与 fail-closed 证据

人工连接使用受管的 `ControlMaster=yes`、`ControlPersist=no` 和私有 `ControlPath`。连接通道确认 master 已认证后，命令 channel 使用既有 socket，不允许回退新认证：

```sh
ssh -F <app>/ssh_config -T \
  -o ControlMaster=no \
  -o ControlPath=<managed-socket> \
  -o BatchMode=yes \
  -o NumberOfPasswordPrompts=0 \
  -o ProxyCommand='exec false' \
  <alias> sh -lc '<approved-command>'
```

`ProxyCommand=exec false` 是回退守卫；ControlPath 缺失、master 退出、session 被拒或 socket 被移除时，命令失败，不会新建 TCP 连接或重新认证。命令字符串会整体 shell quote，避免 SSH argv 重新按空格拆散脚本。

### OpenSSH 门槛记录

历史门槛测试使用 macOS `OpenSSH_9.9p2`、`LibreSSL 3.3.6` 和一个经授权的测试 host alias；本文不记录测试主机的公网地址、用户名或 root 细节。

| 用例 | 结果 |
| --- | --- |
| `-O check` 在认证前/后 | 通过：认证前不就绪，认证后才报告 ready |
| 派生通道与多语句命令 | 通过：沿用人工 master，使用 login directory 语义 |
| 删除 ControlPath socket | 通过：失败且无新 TCP/认证 |
| check 后 master 退出的竞态 | 通过：失败且无新 TCP/认证 |
| MaxSessions 并发门槛 | 通过：服务端拒绝的 channel 明确失败，不回退直连 |

结论：受管复用、`-O check` 就绪证明和 `ProxyCommand=exec false` 守卫共同构成 fail-closed 门槛；普通 `ControlMaster=auto`、`BatchMode` 或预检查单独都不构成证明。

## 客户端配置

以下配置是设置页生成的结构示例。`<CLIENT_ID>` 与 `<PAIRING_TOKEN>` 是 placeholder；真实 token 只在生成后显示一次，复制 JSON 配置后应立即 dismiss。不要把 token 放进 shell command 或 shell history。

### Claude Code `.mcp.json`

```json
{
  "mcpServers": {
    "xtermius": {
      "command": "/Applications/xTermius.app/Contents/MacOS/xtermius-mcp-bridge",
      "args": [],
      "env": {
        "XTERMIUS_MCP_CLIENT_ID": "<CLIENT_ID>",
        "XTERMIUS_MCP_TOKEN": "<PAIRING_TOKEN>"
      }
    }
  }
}
```

### OpenCode `opencode.jsonc`

OpenCode 使用 `mcp` 下的 local server、command array 和 `environment` object。将下面的 `xtermius` entry 合并到既有 `mcp` object，不要覆盖其他 server：

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "xtermius": {
      "type": "local",
      "command": ["/Applications/xTermius.app/Contents/MacOS/xtermius-mcp-bridge"],
      "environment": {
        "XTERMIUS_MCP_CLIENT_ID": "<CLIENT_ID>",
        "XTERMIUS_MCP_TOKEN": "<PAIRING_TOKEN>"
      }
    }
  }
}
```

OpenCode local MCP command shape was checked against the installed `opencode 1.18.31` CLI and its local configuration. Claude Code local `.mcp.json` health checks and OpenCode `mcp list` both reported the signed bridge as connected during this run. These checks negotiated the local stdio server; they did not call tools or send connection metadata to a model. Temporary paired identities were removed afterward and MCP was returned to disabled.

## 打包与分发

- `scripts/build_bridge_sidecar.mjs` 按 target triple 构建 `xtermius-mcp-bridge` 并暂存给 Tauri `bundle.externalBin`。
- 打包后的 macOS bridge 位于 `xTermius.app/Contents/MacOS/xtermius-mcp-bridge`，与 app 一起签名；本机既有验证记录包含 `codesign --verify --deep`。
- `beforeBuildCommand` 已串联 frontend 与 bridge 构建；CI release workflow 为每个矩阵 target 传递 bridge target。
- 本轮在 Intel Mac 本地构建了 x86_64 与 `aarch64-apple-darwin` 两份 `.app`，两份 `codesign --verify --deep --strict` 均通过，主 app 与 bridge 架构分别匹配。arm64 app 无法在当前 Intel 主机运行。
- `tauri build --bundles app` 在两种架构上都完成 app bundle 与 `.app.tar.gz` 后，因本机未注入 `TAURI_SIGNING_PRIVATE_KEY` 而以 updater artifact 步骤退出；发布签名需由 CI secrets 完成。
- `v0.3.7` tag 与 `feat/mcp-v1` 分支已推送。首次 [release CI run](https://github.com/jmluang/xTerm/actions/runs/35867860471) 中，macOS arm64 sidecar 构建及 Rust 测试通过；release gate 的 Clippy 1.98 检出 1 条 `unnecessary_sort_by` 和 5 条 `useless_borrows_in_formatting`，未进入 app bundle 构建。该失败已由本次后续提交修正；v0.3.7 tag 保留不变。

## 本轮性能基线

本机样本使用 macOS 15.7.4、x86_64、release `.app`，每组预热 15 秒后采 40 个 RSS/CPU 点。它们是一次本机对比，不是 SLA。

| App 状态 | 平均 RSS | 峰值 RSS | 平均 CPU | 峰值 CPU |
| --- | ---: | ---: | ---: | ---: |
| MCP disabled | 93,444.6 KiB | 93,528 KiB | 0.565% | 6.8% |
| MCP enabled, idle, no clients | 93,201.8 KiB | 93,344 KiB | 0.915% | 6.2% |

Release-only ignored Rust baselines（逻辑 256 MiB 输出，8 个连接、reader 暂不消费）记录到本轮终端输出：保留 7,864,320 bytes（≤8 MiB），buffer push 测得约 7,964 MiB/s；10,000 次本地 authenticate/authorize/idempotent retry 的 p50/p95/p99 为 4/5/31 μs。该负载绕过 IPC rate limiter，不含 SSH、审批或远端命令成本。持续真实 SSH 输出的 app RSS/CPU 本轮未测：当前运行环境没有 GUI 合成输入授权，未自动打开主机连接。

## 验收状态

| 验收项 | 当前记录 |
| --- | --- |
| 后端单元/针对 MCP 的状态、授权、fail-closed、audit 测试 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib --bins --tests`：137 lib passed + 2 ignored，4 bridge unit + 2 credentials integration passed；另有 2 个显式 ignored performance baselines 已单独 release-run |
| 真实 OpenSSH fail-closed 门槛 | 已有上表 T1–T5 记录 |
| Claude Code + OpenCode 本地 stdio 健康检查 | 两 CLI 均报告签名 bridge connected；本轮未调用工具或模型，完整 tool-call E2E 待做 |
| MCP 关闭/空闲性能 | 本轮 app RSS/CPU 样本见上表 |
| 持续真实 SSH 输出性能 | 尚未测；GUI 合成输入受系统权限限制 |
| macOS x86_64 + arm64 本地 app bundle / deep signature | 两份 app 均构建成功，签名验证通过；arm64 仅作交叉构建验证 |
| macOS 双架构 release CI 构建 | v0.3.7 首轮未通过（见上方 run）；已修正 Clippy 阻塞，新的 release tag 尚待 CI 复验，暂不能标记双架构通过 |
| app 设置页 pairing/config UI 与 audit command | 功能与命令已实现；真实 GUI pairing/copy/dismiss/approval 流程仍待人工或获授权 GUI 验收 |

## 文档状态纠正

`docs/MCP.md` 现在通过 `.gitignore` 的显式例外作为可提交的 canonical 文档。此前 Linear 评论中“docs 已存在”的说法不代表一个可提交的 tracked deliverable；验收应以本文件及其当前代码证据为准。

## V1 明确不做

公网或跨设备入口、自动连接新机器、接管人工 PTY、密码/私钥/MFA 代答、SFTP、本地任意进程代理、命令黑名单 sandbox，以及对同一 OS account 恶意本机进程的隔离，不属于 V1。

# Continuo

**工作在不同 agent、身份和设备之间，持续接续。**

Continuo is an open-source, local-first workspace for identities, tasks, capabilities, MCP connections and sessions across agents and devices. Users bring their own synchronization storage. No Continuo-hosted backend is required.

这是独立的新项目。首版仅接入 **Claude Code、Codex、Hermes**，以适配层隔离运行时差异。桌面端、CLI、MCP stdio 接口共享同一个 Rust 应用服务，agent 可以通过机器接口操作工作空间。

## 当前状态

这是 `0.1.0` 开发原型，尚未发布安装包。

| 已实现 | 范围 |
| --- | --- |
| 六类对象 | 身份、任务、能力、MCP 定义、会话、设备的本地登记与版本管理 |
| CLI | JSON 输入输出、稳定错误码、乐观并发检查 |
| MCP stdio | 初始化、工具发现、结构化工具结果、按启动授权暴露工具 |
| 三种 agent 适配器 | 能力声明、启动/原生恢复参数计划、MCP 注册文档数据 |
| Git 同步 | 用户自选 GitHub 仓库、加密事件、断网本地修改、并发冲突、删除传播 |
| 桌面端 | 六领域管理、版本冲突选择、启动计划、同步配置与手动同步 |

原型目前不执行 agent 进程、不投递 Skills 或改写原生配置，不读取现有账号与会话日志。原生恢复参数需要目标运行时和本机数据支持；跨 agent 接续输出工作材料，不迁移内部状态。设备记录不等于设备授权，首版尚未实现配对、密钥恢复/轮换、细粒度权限和实时会话同步。

## 构建

需要 Rust 1.87+、Git；桌面端另需 Node.js 22.12+ 及 [Tauri 平台依赖](https://v2.tauri.app/start/prerequisites/)。

```sh
cargo test --workspace
cargo build -p continuo-cli
./target/debug/continuo --help

cd apps/desktop
npm ci
npm run build
npm run tauri dev
```

默认数据目录为 `~/.continuo`；所有入口均可用 `CONTINUO_DATA_DIR` 指定同一个目录，CLI 另支持 `--data-dir`。应用代码、同步仓库和本地数据目录分别保存。

## 从 CLI 开始

CLI 的 `call` 接收一个 JSON 对象，输出统一的 JSON envelope。建议通过 stdin 或文件传入复杂文本。

```sh
./target/debug/continuo --data-dir /tmp/continuo-demo status
./target/debug/continuo describe
./target/debug/continuo agents
./target/debug/continuo --data-dir /tmp/continuo-demo call entity.create --input - <<'JSON'
{"kind":"task","name":"开始一个项目","data":{"status":"active","goal":"验证双设备接续","next_steps":["配置同步存储"]}}
JSON
```

`entity.update` 与 `entity.delete` 必须传入先读到的 `expected_revision`。冲突对象使用 `entity.resolve`，提交全部当前版本作为 `expected_heads`。不要把 API key、登录 token 或密码放进对象内容；保存 `credential_ref` 等引用。

## 让 agent 调用

```sh
./target/debug/continuo --data-dir /path/to/continuo-data mcp
```

默认只读。由用户在启动配置中添加权限：

- `--allow-writes`：允许工作空间记录变更。
- `--allow-sync`：允许同步到已配置的远端，需要 `--allow-writes`。
- `--allow-admin`：允许生成本地密钥和修改同步配置，需要 `--allow-writes`。

例如，Claude Code 的 MCP 配置片段：

```json
{
  "mcpServers": {
    "continuo": {
      "type": "stdio",
      "command": "/absolute/path/to/continuo",
      "args": ["--data-dir", "/absolute/path/to/data", "mcp", "--allow-writes"]
    }
  }
}
```

Codex 与 Hermes 的注册结构由 `agent.mcp_registration` 生成。它返回目标格式及文档数据，供客户端或 agent 审查后使用；当前不自动写入原生配置。

## 配置 GitHub 同步

准备一个**用户自己的专用私有仓库**，以及本机 Git 对该仓库的访问权限。支持 GitHub HTTPS/SSH 地址；凭据交给用户的 Git 认证机制管理，不嵌入仓库地址。同步只更新 `continuo-sync` 分支，不使用强制推送。

```sh
# 在仓库之外创建密钥文件。文件已存在时会失败。
./target/debug/continuo call sync.key_generate --input - <<'JSON'
{"path":"/absolute/private/path/continuo.key"}
JSON

./target/debug/continuo call sync.configure --input - <<'JSON'
{"remote":"https://github.com/you/continuo-data.git","key_file":"/absolute/private/path/continuo.key"}
JSON

./target/debug/continuo --allow-sync call sync.run
```

另一台设备配置同一个仓库和同一把密钥，再同步。通过安全的独立渠道转交密钥；密钥不进入 Git。当前密钥文件需为 32 字节，Unix 权限为 `600`，管理界面尚未提供配对向导。失去密钥将无法解密已有数据。

同步存储仅包含格式清单与加密事件。事件正文采用 ChaCha20-Poly1305；设备间冲突按父版本关系识别，不靠时钟判胜负。API key 等登录凭据仍留在设备上。Git 可见提交时间、事件数量和随机标识；此原型尚未完成密码学和恶意存储审计。

GitHub 同步是最终一致、手动触发的工作记录同步。高频会话流、大型工作文件和实时远程执行另行设计。Git 历史保留已删除对象的加密版本，tombstone 不表示物理擦除。

## 项目结构

```text
crates/continuo-core/       领域事件、存储、服务接口、agent 适配器、同步
crates/continuo-cli/        CLI 与 MCP stdio 传输
apps/desktop/              React 界面与薄 Tauri 桥接
docs/                      产品边界、架构、接口、同步协议与决策
```

扩展前请阅读 [架构](docs/architecture.md)、[Agent 接口](docs/agent-api.md)、[同步协议](docs/sync.md) 与 [决策记录](docs/decisions.md)。

## 许可

MIT，见 [LICENSE](LICENSE)。参考来源及代码借用规则见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。Continuo 名称尚未完成商标或域名可用性检索。

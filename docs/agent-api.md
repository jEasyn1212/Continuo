# Agent 接口 v1（开发原型）

应用服务是机器接口真源。运行 `continuo describe` 获得完整输入 schema，MCP `tools/list` 获得当前授权的工具。MCP 工具名是 `continuo_` 加方法名，点号替换为下划线。

| 方法 | 用途 | 权限 |
| --- | --- | --- |
| system.describe | 当前入口获授权的方法、参数 schema 与权限标记 | 读 |
| system.status | 设备、数量、冲突与同步状态 | 读 |
| entity.list / entity.get | 查询六种实体与全部当前版本 | 读 |
| entity.create | 创建记录 | 写 |
| entity.update | 以 expected_revision 更新名称或完整 data | 写 |
| entity.delete | 以 expected_revision 创建 tombstone | 写 |
| entity.resolve | 以全部 expected_heads 创建合并版本 | 写 |
| agent.list | 查询三种适配器的能力与限制 | 读 |
| agent.prepare | 准备启动或原生恢复的 argv | 读 |
| agent.mcp_registration | 生成目标 agent 的 MCP 注册文档数据 | 读 |
| session.handoff | 从明确的任务记录生成跨 agent 接续材料 | 读 |
| sync.key_generate | 在指定路径创建不覆盖的密钥文件 | 写 + 管理 |
| sync.configure | 指定 GitHub 仓库与本地密钥路径 | 写 + 管理 |
| sync.run | 与已配置存储交换加密事件 | 写 + 网络 |

MCP 默认只读，由用户配置 `--allow-writes`、`--allow-sync`、`--allow-admin`。后两者需要写权限。请求参数不能修改入口权限。配置和密钥生成可在用户显式授予管理权限后由 agent 调用。

CLI 默认允许本地记录与管理操作；网络同步仍需 `--allow-sync`。桌面入口由本机用户操作，启用这些权限。Web 默认只读，启动本机服务时按 MCP 相同参数授权。浏览器请求经本机 MCP 子进程调用，不能从 JSON 参数授予自己写入、管理或同步权限。

当前只有入口级权限，还没有身份/对象级 ACL；不要把它当作任意不可信 agent 的安全沙箱。

```json
{"api_version":"1","ok":false,"error":{"code":"revision_conflict","message":"Entity changed, was deleted, or has unresolved conflicts","details":{"current":{"id":"…","heads":[]}}}}
```

推荐 agent 流程：查询实体 → 提取 head.revision → 提交明确变更 → 遇到 revision_conflict 重新读取 → 冲突存在时呈现给用户或按用户授权合并 → 显式同步。

实体 `data` 当前为 JSON 对象，更新会整体替换它，所以保留需要的已有字段。常用内容：identity.instructions；task.goal/status/decisions/next_steps/artifact_refs；capability.source/version；mcp.transport/command/credential_refs；session.agent/native_session_id/task_id/identity_id；device.environment_refs。当前尚未对全部跨实体关系进行外键和领域校验。

已知错误：`invalid_params`、`not_found`、`revision_conflict`、`permission_denied`、`unsupported_agent`、`credential_not_allowed`、`sync_not_configured`、`sync_busy`、`incompatible_workspace`、`decryption_failed`、`sync_push_failed`。同步失败保留本地事件，重试先重新取远端版本。

MCP 使用标准 newline-delimited JSON-RPC stdio，支持协议版本 `2025-11-25`、`2025-06-18`、`2025-03-26`。必须先 initialize，再 notifications/initialized。stdout 仅包含协议消息。应用错误返回 `isError: true`，同时提供文本与 `structuredContent`；协议错误采用 JSON-RPC error。每条输入最大 1 MiB。

协议参考：[MCP 传输](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)、[工具](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)、[生命周期](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)。

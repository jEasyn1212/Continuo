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
| identity.inspect | 检查身份与能力/MCP 关联，返回具体版本与可用性 | 读 |
| identity.current | 本机当前身份、选择版本与可用状态 | 读 |
| identity.activate | 以身份版本和选择版本切换本机当前身份 | 写 |
| identity.clear | 以选择版本清除本机当前身份 | 写 |
| task.inspect | 查询目标、状态、日志、身份与接续就绪检查 | 读 |
| task.transition | 以当前版本流转状态并记录原因 | 写 |
| task.progress / task.decision / task.artifact | 追加进展/检查、决策/理由、产物/验证说明 | 写 |
| task.handoff | 生成绑定当前版本的任务材料与预检清单 | 读 |
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

实体 `data` 当前为 JSON 对象，更新会整体替换它，所以保留需要的已有字段。常用内容：identity.instructions；task.goal/status/decisions/next_steps/artifact_refs；capability.source/version；mcp.transport/server_key/command_hint/credential_refs；session.agent/native_session_id/task_id/identity_id；device.environment_refs。身份字段、能力/MCP 关联、任务结构、状态变化、任务身份与产物引用已有领域校验；其他对象的关系尚未全部校验。身份字段与机器操作流程见 [身份模块](identity.md)。

`agent.prepare` 默认采用本机当前身份；显式 `identity_id` 优先，`use_current_identity:false` 可在没有显式身份时跳过本机选择。失效的当前身份会报错，不会隐式使用空身份。返回 `identity_context`，包含身份及关联记录的版本快照，能力和 MCP 尚未投递到原生配置。

任务接口详情见 [任务模块](task.md)。`agent.prepare` 可传 `task_id` 与 `expected_task_revision`；使用任务接续材料作为指令，优先采用任务关联的身份。显式身份与任务关联不一致时拒绝计划，调用者应先明确修改任务。`session.handoff` 是 `task.handoff` 的兼容方法名，0.3.0 起也要求目标、下一步与开放任务状态。

已知错误：`invalid_task_profile`、`invalid_task_transition`、`task_transition_requires_reason`、`task_not_ready`、`task_identity_mismatch`、`invalid_identity_profile`、`identity_bindings_unavailable`、`selection_conflict`、`invalid_params`、`not_found`、`revision_conflict`、`permission_denied`、`unsupported_agent`、`credential_not_allowed`、`sync_not_configured`、`sync_busy`、`incompatible_workspace`、`decryption_failed`、`sync_push_failed`。同步失败保留本地事件，重试先重新取远端版本。

MCP 使用标准 newline-delimited JSON-RPC stdio，支持协议版本 `2025-11-25`、`2025-06-18`、`2025-03-26`。必须先 initialize，再 notifications/initialized。stdout 仅包含协议消息。应用错误返回 `isError: true`，同时提供文本与 `structuredContent`；协议错误采用 JSON-RPC error。每条输入最大 1 MiB。

协议参考：[MCP 传输](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)、[工具](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)、[生命周期](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)。

## 能力操作

`capability.import_text` 接受 `name`、`body`，可选 `capability_type` (`rule`/`skill`)、`version`、`source_ref`、`source_revision`、`source_license`，始终创建未检查记录。完整编辑（含 `agent_targets`、`requires`）仍使用 `entity.update` 的 revision CAS。

`capability.inspect {id, target_agent?}` 返回 `digest`、`capability.revision`、`issues` 与依赖快照。随后 `capability.review {id, expected_revision, expected_digest}` 确认这一版正文；此接口需入口写权限，但不授予运行权限。

`capability.prepare {capability_ids, target_agent}` 返回依赖在前的适配器应用。`agent.prepare` 增加 `capability_ids` 供额外选择，并自动应用身份关联能力；输出 `capability_context` 包含 revision/digest/策略/来源与 `scripts_executed:false`、`permissions_granted:false`、`config_written:false`。缺失、冲突、循环、未检查或不适用时返回结构化 `capability_not_ready`。所有方法经共享操作目录暴露为 CLI/MCP/App/Web 接口。

## MCP 定义与本机连接

`mcp.inspect {id,target_agent}` 分别返回 definition、mapping、adaptation、connection、authorization 与 issues。`mcp.map {id,expected_revision,expected_mapping_revision,mapping}` 用双 revision CAS 保存本机 executable/args/cwd/env_refs；需要写与 admin 权限。`mcp.clear_mapping {id,expected_mapping_revision}` 只清除本机映射并保留版本标记。

`mcp.prepare {id,target_agent,expected_revision?}` 生成选定 adapter 的注册文档数据，不写配置。`agent.prepare` 的身份绑定还会返回 `mcp_context`，缺失本机环境时明确 needs_setup；未声称安装或工具授权。

`mcp.probe {id,expected_revision,expected_mapping_revision,probe_id,confirm_execution,timeout_ms?}` 为一次有界 stdio 协议检查，需独立 probes 许可、写/admin 与明确确认；默认入口不暴露。`mcp.cancel {id,probe_id}` 可在检查进行时请求取消。超时 100..5000 ms（默认 2000），不调用工具，原始服务输出不持久化。CLI/MCP 可加 --allow-mcp-probes；MCP worker 还需 --allow-writes --allow-admin。不是程序沙箱，不自动解析凭据或运行导入内容。

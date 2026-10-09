# 身份模块（0.2.0）

身份是可同步的工作角色与指引。账号认证仍属于各 agent 与设备上的凭据管理；操作权限由 Continuo 服务层的入口授权执行。身份指引不构成安全隔离，也不会自动切换登录账号。

## 内容

身份仍使用 `entity.create/update/delete/resolve` 管理，`kind` 为 `identity`，`name` 为显示名称。`data` 的字段：

| 字段 | 类型与限制 | 用途 |
| --- | --- | --- |
| description | 字符串，最多 8 KiB | 角色使用场景 |
| instructions | 字符串，最多 64 KiB | 提供给 agent 的指引 |
| preferred_agent | 字符串或 null，最多 128 字节 | 客户端建议的 agent；当前仅三种适配器可执行计划 |
| capability_ids | 规范 UUID 数组，每类最多 64 个且不可重复 | 关联能力记录 |
| mcp_ids | 同上 | 关联 MCP 定义 |

这些字段可以省略，旧版仅含 `instructions` 的身份仍可使用。界面编辑保留未知扩展字段。机器接口的 `entity.update` 仍整体替换 `data`，调用方必须保留需要的字段。

本地创建、更新与合并会在同一事务中检查关联类型、存在性、删除状态与冲突。同步导入只检查结构，保留缺失关联的历史记录，避免因依赖尚未到齐而丢失数据。`identity.inspect` 返回每项关联的 `ready/missing/deleted/conflicted/wrong_kind` 状态；只有有效关系才能激活身份或生成使用该身份的启动计划。删除失效身份仍被允许，历史保留。

## 当前身份与 agent 接口

1. 调用 `identity.current`，读取 `selection.revision`（首次为 `none`）和设备当前身份。
2. 读取目标身份的 `head.revision`。
3. 调用 `identity.activate`，传入 `id`、`expected_revision`、`expected_selection_revision`。选择修改与目标检查在同一事务内完成。
4. 调用 `agent.prepare`，传入 `agent` 与本机绝对路径 `cwd`，默认采用当前身份。显式 `identity_id` 优先。没有显式身份时，可用 `use_current_identity:false` 跳过本机选择。
5. `identity.clear` 以 `expected_selection_revision` 清除当前身份。多个 App、Web 或 agent 争抢切换时，过期请求返回 `selection_conflict`。

当前选择存在设备本地 metadata，不进入同步事件。其他设备接收相同身份内容，但不会被远端切换当前角色。当前身份更新后，新计划读取最新有效版本；如果身份或关联被删除、发生冲突或缺失，状态变为 `unavailable`，计划报错，调用方应先修复或明确不使用身份。

启动计划带有 `identity_context`：身份事件、标准 profile、关联记录当前版本及来源 `current/explicit`。它是读取事务中的一致快照；生成计划不启动进程，不投递能力或 MCP 配置，也不修改 agent 的账号、权限或内部会话状态。后续执行层应在实际启动前重新确认版本和运行环境。

Claude Code 使用追加系统指引；Codex 使用开发者指引配置；Hermes 在首条用户指令中携带身份上下文。这些是现有适配器的计划能力，仍未实测三家的真实运行时。

## 客户端流程

身份工作区提供搜索、新建、编辑、删除、角色说明与指引、偏好 agent、真实能力/MCP 记录选择、关联检查、当前身份切换，以及冲突版本选取后编辑合并。合并引用全部当前 heads，保留因果历史；过期保存报错，未保存草稿保留，用户可刷新查看新版本后重新编辑。

Agent 页面可以使用当前身份、指定某个身份或本次不用身份。选择有偏好 agent 的身份时，界面建议该 agent；用户仍可改选。App 与 Web 共用界面，所有操作经服务层执行，CLI/MCP 同样可调用。

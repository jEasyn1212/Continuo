# 任务模块（0.3.0）

任务记录明确的工作目标、进展和判断，提供可以检查的接续材料。它与身份、能力、MCP、会话和设备同为核心领域；任务材料不等于迁移 agent 的内部状态。

## 工作流程

创建任务 → 填写目标、验收标准、下一步与可选身份 → 开始工作 → 记录进展和已执行检查 → 留下决策及理由 → 登记产物引用 → 生成接续材料或填写完成结论。受阻、暂停、取消和重新打开是明确的状态变化。

App/Web 共用任务工作区。CLI/MCP 使用相同的 `Service::call` 操作目录。任何写操作都需要先读当前 revision，过期写入被拒绝；输入草稿不会因过期保存自动消失。

## 数据

任务使用 `entity.create/update/delete/resolve`，`kind:"task"`。任务 `data` 包含：

| 字段 | 类型 | 用途 |
| --- | --- | --- |
| goal | 字符串，最多 64 KiB | 要达成的结果 |
| status | backlog / active / blocked / paused / done / cancelled | 状态，省略默认为 backlog |
| success_criteria | 字符串数组 | 验收标准 |
| next_steps | 字符串数组 | 下一次可以执行的工作 |
| blockers | 字符串数组 | 当前阻碍，受阻状态必须填写 |
| identity_id | UUID 或 null | 可选的可同步身份关联 |
| completion_summary | 字符串，最多 64 KiB | 完成结论，新本地完成状态必须填写 |
| progress | 进展条目数组 | summary、checks，以及核心生成的 UUID、设备标识、展示时间 |
| decision_log | 决策条目数组 | summary、reason，以及同类记录元数据 |
| artifacts | 产物条目数组 | title、reference、verification，以及同类记录元数据 |
| decisions / artifact_refs | 字符串数组 | 保留已有简易记录的兼容字段 |

一般字符串列表每类最多 128 项，每项最多 8 KiB；结构化日志每类最多 256 项，摘要/理由/验证说明最多 64 KiB。总事件仍限制为 1 MiB。时间仅用于显示，不决定同步冲突的胜负。每个结构化条目有唯一 UUID。

产物只登记引用，不上传文件。支持 `project:docs/review.md` 这样的相对项目引用，以及无账号、密码、查询参数或 fragment 的 HTTPS URL。拒绝本机绝对路径、file URL、父目录逃逸和带凭据的 URL。项目引用在接收设备的 checkout 中解释，URL 的访问与可用性并未由登记操作检查。

旧记录可以缺少新字段；旧的 done/blocked 记录仍可读取，编辑时需要补齐结论/阻碍。只有短标题或 description 的旧任务，需要明确填写 goal 与 next_steps 后才能生成新接续材料。未知扩展字段在界面和追加日志操作中保留；直接 `entity.update` 仍整体替换 data，调用方需要保留已有内容。

## 状态与记录接口

| 当前状态 | 允许进入 |
| --- | --- |
| backlog | active / paused / cancelled |
| active | blocked / paused / done / cancelled |
| blocked | active / paused / cancelled |
| paused | active / backlog / cancelled |
| done | active（重新打开） |
| cancelled | backlog（重新打开） |

`task.transition` 接收 `id`、`expected_revision`、`status`、`reason`，追加一次包含 from/to/reason 的状态原因日志。受阻时把原因记录为 blocker，恢复 active 时清除 blockers，完成时把原因记录为 completion_summary。一般 `entity.update` 同样校验流转，并要求状态变化附带新的对应 from/to/reason 原因日志，不能绕过领域规则。显式多版本合并可以选择合并后的状态，但仍须满足结构与结论/阻碍约束。

`task.progress` 接收 `summary` 和可选的 `checks` 字符串数组；`task.decision` 接收 `summary`、`reason`；`task.artifact` 接收 `title`、`reference`、可选 `verification`。三者均接收 `id` 和 `expected_revision`，从精确的当前版本构造追加内容并在写事务中再次检查，避免多个入口丢失更新。检查和验证说明是记录者的声明，Continuo 不把它们标记为已独立验证。

`task.inspect` 返回当前任务与标准字段、允许的状态变化、接续问题和关联身份的一致读取快照。关联身份被删除、发生冲突或工具绑定失效时仍能看见任务，但接续会被阻断；可明确修复或解除关联。

## 接续与启动计划

`task.handoff` 接收 `task_id`、`target_agent` 和可选 `expected_revision`，只读地生成版本绑定的 packet；`session.handoff` 走同一个实现。要求目标、下一步、开放状态及有效的指定身份。已有冲突必须先合并。受阻任务可以转交分析，但清单明确要求先解决阻碍才能做依赖它的工作。done/cancelled 任务必须重新打开。

packet 包含目标、验收、状态、进展/检查、决策/理由、产物/验证说明、下一步、任务版本、指定身份及工具关系版本，以及任务版本/设备环境/当前授权/产物检查/阻碍的预检清单。它不复制私有 conversation state，也不自动执行后续动作。

`agent.prepare` 支持 `task_id`、`expected_task_revision`，重新核对任务后构造同一材料作为 prompt。任务指定身份优先；没有指定时沿用显式身份或设备当前身份。显式身份与任务指定身份不一致时拒绝，调用者应先明确编辑任务关联。Claude Code、Codex、Hermes 通过已有适配器生成参数计划，仍不启动进程或写原生配置。后续执行层应在实际启动前重新核对版本与环境。

并发记录在同步后保持多个 heads。界面提供以某版为起点编辑合并，并可合并各版本新增的日志/产物；同一日志 ID 有不同内容时拒绝自动拼接，要求明确处理。合并引用所有当前版本，历史保留，不按墙上时钟挑选胜者。

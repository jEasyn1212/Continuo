# 会话模块（0.6.0）

会话记录连接 agent、身份、任务、来源设备和环境。它是用户提供的引用和摘要，不是账号历史导入或进程管理器。

## 登记与关联

`session.register {name,data}` 登记 `agent`、可选 `native_session_id`、`task_id`、`identity_id`、`origin_device_id`、`environment_ref`、`source`（manual/referenced）、`source_ref`、`summary`。新记录未填写来源设备时登记当前设备 UUID；UUID 是来源说明，不是设备认证。环境和来源引用使用 project:相对引用或无凭据/查询参数的 HTTPS，引用不会自动读取。原生 ID 仅为可同步标识，不包含原生历史或恢复所需状态。

旧的空会话记录作为待完善记录保留。已有未知 agent 标识仍可作为来源元数据；当前运行时适配器严格只有 Claude Code、Codex、Hermes。

本地保存检查任务/身份是否有效，会话显式身份不能与任务指定身份冲突。同步导入只校验字段形状，允许缺失关联保留并修复；并发版本需要显式合并全部 heads。记录不保存本机 cwd/executable/env、账号引用或原始 messages/transcript/runtime_state。自由摘要仍需用户避免填写秘密和内部原始内容。

`session.inspect {id}` 返回逻辑关联、原生恢复检查项、接续检查项、当前设备映射与状态日志。任务/身份被删除或出现冲突后，相关计划会明确阻断。

## 本机原生恢复

`session.map {id,expected_revision,expected_mapping_revision,mapping}` 需要写与管理权限。映射保存本机 executable、cwd、可选 account_ref（credential:/keychain:/env: 引用）、用户记录的 runtime_version、state_present_confirmed。账号引用不解析、不登录、不改变权限；路径和确认不进入同步。

确认标记表示用户已核对本机及所用账号下存在原生会话，不是 Continuo 独立验证。修改 agent、原生 ID、来源设备或环境引用会使映射的绑定摘要失效，需重新核对。修改摘要或记录状态不会假称原生状态已经改变。映射保存用双 revision CAS；清除保留本地 tombstone 防止旧请求覆盖。

`session.resume_plan {id,expected_revision,expected_mapping_revision}` 检查可执行文件、工作目录、本机确认和关联，再由 AgentAdapter::prepare_resume 生成原生恢复参数。仅准备 argv，不执行，不发现其他账号的会话、不读取内部历史、不自动重写原会话的身份提示。计划标明 native_resume、executed:false、身份指引未重新应用、登录未核验、内部状态未迁移。

## 跨 agent 接续

`session.packet {id,expected_revision,target_agent,expected_task_revision}` 需要开放的会话记录、明确且可接续的任务与可用身份。它在同一读取事务中固定会话/任务/身份版本，生成任务进展、决策、产物、检查清单和会话摘要；不把原生 ID 传给目标 agent。

`session.continue_plan` 另需目的设备 cwd，复用 agent.prepare 与能力/MCP 预检，生成新会话计划。与原生恢复分开：不带 resume 参数，不安装配置或启动 agent，也不承诺完整迁移上下文、账号或内部状态。任务/会话/身份版本变化会要求重新核对。

## 状态与取消

状态为 registered/active/paused/closed/cancelled。`session.transition {id,expected_revision,status,reason}` 校验允许的变化并追加原因；结束/取消后需要明确重新打开记录，才可生成计划。状态全部是用户记录，不证明进程正在运行；取消不会杀掉实际 agent 进程。

App/Web 共用完整表单，支持登记/关联、状态原因、本机环境、两个计划流程、取消编辑、过期保存保留草稿、并发版本整理与删除标记。只读入口能检查与准备计划，不能修改记录或设备映射。没有执行中的 agent 会话作测试夹具。

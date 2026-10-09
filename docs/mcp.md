# MCP 工作区（0.5.0）

MCP 工作流依次为：创建可同步定义 → 配置本机命令 → 核对环境与目标 adapter → 用户确认一次连接检查 → 生成注册文档数据。保存定义、适配成功、连接成功和获准执行检查是不同状态，不能互相替代。

## 定义与设备映射

可同步定义保存说明、`server_key`（agent 注册键）、`transport`、不含本机路径的程序提示、HTTPS endpoint、所需环境变量名称和凭据引用。定义拒绝 command/args/cwd/env、mapping 与连接状态等设备字段，避免把原生配置误存为同步定义。stdio 可用；HTTP 定义可以保存，但连接与注册适配尚未实现，不会假装已连接。旧的空定义仍作为待完善记录保留。

`mcp.map` 保存本机绝对 executable、字面 argv 数组、可选 cwd、`NAME -> env:VARIABLE` 引用。映射存于本地 metadata，不进入同步事件，也不复制给其他设备。保存需要同时匹配定义 revision 和本机 mapping revision。清除映射保留新的本地版本标记，防止旧的“从未配置”请求在清除后覆盖配置。定义删除/冲突不会强行删除本机映射，修复后可继续核对。

敏感值需单独保管。当前尚未实现凭据解析；有环境/凭据引用的记录会明确阻止检查与注册，不输出占位符假装配置可用。参数也不能填写秘密，正文/argv 的自由文本不能靠结构键检查证明没有秘密。

## 检查、授权与取消

`mcp.inspect` 分别返回定义、本机映射、环境问题、目标适配结果、连接状态和接口授权。exe 缺失/无执行权限、cwd 缺失、环境引用缺失、HTTP/凭据解析尚未支持等均可见。检查结果绑定定义与映射的精确 revision；修改后显示 stale。孤立的 running 状态通过本机锁检查显示 interrupted，可重新检查。

运行 `mcp.probe` 需要独立执行许可，以及 `confirm_execution:true`、两个精确 revision、新的 UUID `probe_id`。Web 和 MCP worker 默认不提供执行许可；只有用户主动加 `--allow-writes --allow-admin --allow-mcp-probes` 后才暴露检查工具。单次 CLI 调用可显式加 `--allow-mcp-probes`。不会把原有 admin 权限自动视为执行许可。

App 在用户确认明确显示的 executable/argv 后，只对该次原生请求授予检查许可，不存储持续授权。API 控制台、CLI 和获准的 agent 调用者必须自行审查命令并明确提交确认；该参数不是人类专属审批证明。来源导入、身份选择、查看定义、保存映射或生成注册计划不会触发检查。

检查默认总协议超时 2 秒，可选 100..5000 ms；使用干净环境，不继承私人环境变量。只发送 initialize、initialized 和（若服务声明支持）tools/list，回应基本 ping，不调用 tools/call、资源读取、采样、文件根授权或其他扩展。工具数量只代表读取到的页面；不会存储服务的原始输出、指令或错误文本。初始化版本不兼容、错误响应、非法/超大输出、服务退出和超时会显示失败原因，可修改后重试。

`mcp.cancel` 只取消匹配 `id + probe_id` 的当前检查。CLI/MCP 的有界检查在独立 worker 执行，其他请求仍可取消或读取状态；App 每次请求使用独立 core connection，避免检查占住取消入口。检查 UUID 不能重用。最多同时 4 个 MCP worker 检查，每个定义另有本机锁，阻止并发启动同一服务。

结束时关闭 stdin，并终止本次创建的 Unix process group。当前仅在 macOS/Linux 启用检查；Windows 的有界进程清理尚未实现。协议检查不是程序沙箱：被用户明确批准的程序本身仍可能访问文件、联网或产生副作用；脱离进程组的后台守护进程不受这项机制保证。Continuo 被强制关闭或崩溃时，进程清理不能保证完成，需核查残留程序后再尝试。不执行来源不明的服务作实测。

[官方 lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) 与 [stdio transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) 是协议实现参考。客户端没有复制 SDK 或参考服务源码。

## 注册与身份计划

`mcp.prepare` 调用 `AgentAdapter::prepare_mcp`，生成 Claude Code JSON、Codex TOML 或 Hermes YAML 的文档数据与可复制的 `native_text`（YAML 使用 JSON 兼容写法）；需要有效定义与本机映射。仍未验证用户本机 agent 登录/版本，也不写个人配置。当前三个格式未安全实现 cwd 表达，有 cwd 时明确返回适配限制；可清空或用用户自己审查的启动器。

绑定身份后的 `agent.prepare` 包含 `mcp_context`，展示已准备或 needs_setup 的注册状态与精确版本；不会把绑定等同于装入原生 agent。注册键冲突、配置安装、HTTP/OAuth、凭据解析和细粒度工具许可需后续真实集成，必须获得适用授权后推进。

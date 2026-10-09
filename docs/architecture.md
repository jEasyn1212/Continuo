# 架构

```mermaid
flowchart TD
    CLI[CLI] --> S[应用服务与接口目录]
    MCP[MCP stdio] --> S
    UI[共享 React 界面] --> T[Tauri 原生桥接]
    UI --> W[127.0.0.1 Web 本机桥接]
    T --> S
    W --> MCP
    S --> P[入口权限与输入校验]
    P --> E[领域事件与本地 SQLite]
    P --> R[AgentAdapter 注册表]
    R --> C[Claude Code]
    R --> X[Codex]
    R --> H[Hermes]
    E --> G[加密 Git 同步]
    G --> U[用户自己的 GitHub 仓库]
```

`Service::call` 是共同业务入口。`operations()` 保存方法名称、JSON 输入 schema、读写/网络/管理标记和 MCP 工具名称。所有入口复用它，不重复实现业务规则。`system.describe` 返回当前入口获授权的目录，App/Web 接口控制台从它或 MCP tools/list 生成操作列表。输出统一为 `{api_version, ok, data|error}`，错误具有稳定的 `code`，必要时包含结构化 `details`。

本地记录由不可变事件构成。一个事件引用之前的父版本；没有被后继事件引用的版本为当前 head。一个实体有多个 head 时存在冲突。`update` 要求调用者提供唯一当前版本，`resolve` 要求提供全部当前版本。SQLite 事务将新事件一次提交，派生视图可以从事件重建。

设备 UUID 是事件来源标识，不是身份认证。事件时间用于显示，不参与冲突胜负。当前只有一个本地 vault，尚无多租户或细粒度资源授权。

`AgentAdapter` 有基础接口：

- `descriptor()`：稳定标识、可调用能力、实际实现的限制。
- `prepare_launch()`：返回 executable、argv、cwd、env 和告警，不执行 shell 拼接。
- `mcp_registration()` / `prepare_mcp()`：返回注册文档，不自行改写配置。
- `prepare_capability()`：准备检查后的能力文本。
- `prepare_resume()`：准备本机原生恢复参数，默认实现明确不支持；不读取会话历史或执行进程。

添加新 agent 的步骤：实现 trait、登记适配器、增加契约样例并验证目标运行时版本。协议入口和领域存储不应因此改变。未来加入 `probe / inspect / plan_apply / apply / rollback / discover_sessions` 等明确接口时，先定义共同 DTO 和错误，再由适配器实现，不能以一个“支持 agent”布尔值代替细粒度能力。

运行时接口的 agent 参数枚举来自服务的注册表，未安装的适配器不能被调用。会话记录保留 agent 标识作为可迁移元数据，不要求每台设备都支持该运行时，也不会因此执行外部程序。默认注册表严格只有 Claude Code、Codex、Hermes。

当前 Claude Code 身份材料使用 append-system-prompt 参数；Codex 使用 developer_instructions 覆盖；Hermes 使用初始用户消息。它们的语义有差异，描述中明确声明。当前计划基于官方文档和本机 CLI/source 核对，未执行真实会话。原生恢复也只准备本机参数，不说明远端状态已迁移。

Tauri 桥接在 blocking worker 上调用服务。桌面、CLI 与 MCP 默认使用同一个数据目录，也可以显式指定。Web 同一套页面采用同源 HTTP → MCP stdio → Service::call；Node 桥接仅提供静态资产和协议传输，不实现领域规则。它只监听 127.0.0.1，不提供公共 HTTP API 或 Continuo 同步服务。

同步当前通过 `SyncBackend` 的实验性 Git 实现。新增第二种存储前，需将读取远端快照、发布事件和条件提交细化为传输契约，保持加密、因果合并和冲突处理在共同引擎中。

Web 启动时创建一次性内存 token；页面从同源 bootstrap 获取它，API 请求携带 Authorization 和固定客户端头。服务检查 Host、Origin、内容类型、请求大小与静态文件真实路径；不开放 CORS。权限在启动 MCP 子进程时指定，所有操作仍经过 Rust 服务校验。token 不放入 URL、日志或浏览器持久存储。该边界防止跨站浏览器请求，不隔离拥有本机访问权限的其他进程。

Web 的资产和本机桥接可在没有 Tauri App 的情况下独立使用，需要 Node、CLI 二进制与构建后的 HTML/CSS/JS；App 不需要 Node 或 Web 服务才能运行。当前尚未提供自动安装、后台托管和签名分发工作流。

## 受管 home 与本机模拟生命周期

prepare_managed_config 契约由 adapter 生成单个 MCP 注册文件及相对配置根；通用 deployment 服务生成新 home 并以本机 CAS 指针选择，保留历史。进程服务通过同一 Service::call/catalog，仅接收接口注入的自身二进制，不接受来自 API 的 executable/argv/cwd；它消费已验证 home/environment，却不会执行 adapter 的真实 agent 计划。

本机 metadata 保存 run/control_revision/受限输出，文件锁表明独立监管者是否存在。监管者用自己的 Child 句柄停止与回收；接口退出不结束监管者，监管者崩溃则固定模拟 worker 因 stdin EOF 退出。恢复只在锁释放后确认 interrupted，不凭保存 PID 接管、杀进程或自动重试。home、进程、环境与输出不进入同步事件；这些机制不构成安全沙箱或账号隔离。详见 [生命周期契约](managed-processes.md)。

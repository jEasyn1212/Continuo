# 下一里程碑：受管配置投递与回滚

这是 0.7 后的源码阶段，先补配置闭环。仅投递单个已有 MCP 定义的注册文件到 Continuo 自己生成的 home，不合并或改写个人 home，不执行 agent，不处理账号、凭据、密钥或同步访问。

## 最小交付

- adapter 新增 prepare_managed_config 契约。当前三个 adapter 返回文件内容、相对路径与显式配置目录；默认实现明确不支持。通用投递服务不按 agent 名散落分支。
- deployment.plan 预览当前 MCP 定义/本机映射生成的确切文件及 plan_digest。
- deployment.apply 需要写/admin、明确确认、相同 plan_digest 和当前选择 revision。重新核对定义/映射版本，文件先完整写入新 UUID generation，最后在 SQLite 事务中发布本机当前指针；失败不替换旧指针。
- deployment.inspect 显示文件完整性、当前 home 与本机选择版本。
- deployment.rollback 选择已有完整 generation，产生新的选择 revision。旧 home 和外部改动都保留，不复制运行时状态，不停止进程。被破坏的当前 home 可以退出选择，但目标 home 必须通过完整性检查。
- deployment.launch_plan 对当前文件、源定义和映射复核，返回 argv 和 inherit_env:false 的显式环境计划。执行仍未实现；调用方必须按计划清空继承环境，不能仅覆盖几个变量。

路径仅能落在当前 vault 的 managed-homes/<adapter>/generations/<UUID>，不接受个人 home 参数。目录/文件的符号链接与硬链接受检查，Unix 目录 700、文件 600。不是同用户恶意进程的安全沙箱。每个 adapter 暂只有一个当前选择；第一份配置没有旧 generation 可回滚，尚无清空选择入口。暂不清除旧 generations；它们可能在未来运行后含状态，不能作为同步或备份配置上传。

原子性限定：SQLite 指针切换原子，发布前同步文件与目录（Unix）。写入失败保留旧指针；发布前崩溃可能留下未选中的 generation。它不是对运行中 agent 的热重载，也不承诺停进程或完整运行时回滚。回滚后的配置若已与领域定义/映射脱节，启动计划仍阻止使用。

## adapter 来源与验证边界

Claude Code 将生成的 .claude/.claude.json 配合 CLAUDE_CONFIG_DIR 使用，Codex 使用 .codex/config.toml 与 CODEX_HOME，Hermes 使用 .hermes/config.yaml 与 HERMES_HOME。依据官方资料核对路径，不复制上游代码。Hermes 使用 JSON 文本这一 YAML 子集；不写 auth.json、.env、账号或信任/权限设置。

来源（2026-10-09 核对）：
- [Claude Code MCP 配置](https://code.claude.com/docs/en/mcp-quickstart)
- [Codex 配置目录](https://learn.chatgpt.com/docs/config-file/config-advanced)
- [Hermes 配置与 home 模式](https://hermes-agent.nousresearch.com/docs/user-guide/configuration/)

独立 HOME/配置目录只减少意外配置继承，不隔离 OS keychain、共享账号、项目配置、文件系统或网络。返回 security_isolation:false、account_authentication_verified:false、credential_files_copied:false、credentials_resolved:false，现阶段不能称作账号隔离。只采用所选 MCP 映射的命令/argv，不读取凭据文件；用户写入 argv 的自由文本秘密无法自动检测，不能据此声称配置文本绝无秘密。

## 能在隔离夹具中验证

临时 vault/home、三个 adapter 的 JSON/TOML/YAML 子集、版本与正文变化阻止旧计划、CAS、两次投递/回滚、外部文件变动保留、符号链接拒绝、写入故障不改变指针、目录/文件权限。仅运行 /bin/sh 模拟程序检查显式环境与假配置存在；没有运行已安装的 Claude Code、Codex 或 Hermes。Web 经真实 MCP/Rust 服务，CLI 读取相同本机选择，权限仍在服务层校验。

## 必须另行确认

实际原生运行：agent/版本、可信 executable、项目 cwd、账号环境、允许的文件/网络/工具范围、运行时间/停止策略和完整命令。
个人配置迁移/合并：明确文件、可改字段、备份位置、冲突/回滚范围；本阶段接口没有此路径。
账号：登录/登出、钥匙串或秘密引用读取、重新授权均未实施。任何用于建立持续访问的新凭据、同步密钥生成/转交/配置，仍需专门批准或用户交接，不能由开发授权推导。

真实私有仓 Mini/Air 同步继续等待主对话回复；本阶段无需它。项目仍在进行，不把配置生成、模拟执行或参数计划作为真实原生运行验收。

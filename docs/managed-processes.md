# 受管配置与模拟生命周期

这是 0.7 后的有限源码里程碑，版本仍保留 0.7.0。App/Web 的「受管运行」页、CLI、MCP 使用同一核心接口。只有 Continuo 自带模拟程序会运行：三个 adapter 决定 home 格式，但其 agent 启动计划和 MCP 命令均不执行。没有读取账号、私人 profile、日志、凭据或真实私有同步仓。

## 可演示的端到端路径

1. 构建 CLI 和 renderer，选择隔离空 vault，启动仅监听本机的 Web。默认启动不会开放模拟执行。要验证本里程碑，用户明确加 `--allow-writes --allow-admin --allow-managed-processes`；不需要 sync/probes。App 的每次启动模拟程序也需确认，不保留执行许可。
2. 在 MCP 模块新建 stdio 定义，server_key 使用无空格名称。本机映射指向本项目构建的 `target/debug/continuo` 绝对路径，args 为 `["mcp"]`。这是真实的可执行 MCP 入口；本流程只把注册写入生成文件，不执行该命令。不得换成未经审查的服务或将秘密填入参数。
3. 「受管运行」选目标 agent 与该定义，预览文件，确认投递。生成 home 显示完整性和选择 revision。再预览/投递一版后，可回滚上一选择；各 generation 保留，不覆盖或复制运行状态。
4. 设置模拟时长与超时，确认启动。查看本机 run UUID、状态、监管锁、stdout/stderr、退出码或信号。选择较长时长可点击停止；超时小于时长可演示超时回收。单个 agent 有未结束运行时不能新启动、投递或回滚配置。
5. 关闭或重启 CLI/MCP/Web 接口进程后，监管者仍独立运行；新入口可查询并请求停止。模拟程序最长10秒，超时上限15秒。监管者崩溃时，模拟程序因 stdin 管道关闭退出；重新查询显示锁已释放。显式「确认记录中断」只将本机记录变成 interrupted，不按保存 PID 杀进程或接管，也不自动重试。

CLI 示例：通过 `continuo --data-dir <隔离vault> call deployment.inspect --input -` 取得选择版本。构造 process.start JSON 后，以 `call process.start --allow-managed-processes --input -` 提交；随后用 process.inspect 查询，再用 process.stop 带精确 control_revision 请求停止。MCP 工具分别为 continuo_process_start / inspect / list / stop / recover；参数与界面相同，详见 [共享 API](agent-api.md)。接口控制台调用也需明确确认字段和本入口已有许可，不能靠 JSON 自行打开 Web 执行能力。

## 生命周期契约与故障边界

- 启动在事务中核对选择、文件完整性及源 MCP 定义/映射，写 starting 记录后启动自身二进制的监管角色。失败或未完成的启动可查询，不自动重试；延迟监管者必须检查记录仍为 starting 才能创建子进程。
- 监管者持有独立运行锁，以自己的 Child 句柄管理子进程。stdin 用于合作停止和监管者消失检测；超过短暂宽限后，只向仍持有的子进程组发送 TERM，再对该 Child 发送 KILL。终态在回收后发布。旧 PID 仅供显示，恢复没有向它发信号的路径。
- 监管锁不是设备信任或安全沙箱。刚启动时短暂未持锁不等于已崩溃；先刷新再决定是否记录中断。显式恢复用锁与 control_revision 防止接管活跃运行。恢复的 termination_verified 保持 false，不宣称已验证旧进程结束。
- stdout/stderr 各显示至多4096字节，继续排空管道并记录总字节数，避免输出洪泛阻塞。仅为固定模拟输出，未承诺任意程序、脱离子进程或后台服务的生命周期管理。
- home、环境、进程、输出和锁均为设备本地状态，不进入领域事件或同步包。clear environment 与独立配置目录不隔离文件系统、网络、钥匙串或项目配置。
- 当前生命周期仅支持 macOS/Linux。运行历史最多返回100条，尚无清除/分页入口；SQLite 可以继续积累本机历史。无自动重启、真实 agent 执行、账号认证、会话发现或完整内部状态迁移。

## 验收边界

隔离测试覆盖三种 home、确认/权限、源版本、输出限制、退出码、停止/超时、接口父进程重启、监管者崩溃、stdin 丢失退出、保存 PID 被复用时的安全恢复、App/Web 共用表单和 CLI/MCP 互操作。临时 bare Git 回归继续验证同步；没有利用用户仓库或日常 profile 做夹具。

源码与构建通过不能替代 Air App/Web 窗口验收。Air 当前仍为之前的0.7演示包，本源码里程碑尚未部署。真实原生验收需要用户指定 agent、可信 executable/版本、项目目录、账号与文件/网络范围；真实双设备同步需要已有专用私有仓 URL、新隔离目录、密钥交接的具体批准。屏幕解锁须由用户进行。缺少这些验收时先收尾现有接口、测试和文档，不追加其他功能。

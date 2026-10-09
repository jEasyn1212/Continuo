# 验证记录

2026-10-09，macOS arm64，Rust 1.87.0 / Node.js 22.23.2。

已通过：

- `cargo test --workspace --locked`：10 项集成测试，使用临时数据目录和临时 bare Git 仓库。
- 两个独立本地节点的创建、离线并发修改、加密上传/下载、冲突合并、重复同步、删除传播。
- 错误密钥与不完整事件图不会部分导入；加密拒绝密文篡改和文件名替换。
- 本地同时修改同一版本，只有一个成功，另一个获得 revision_conflict。
- CLI JSON 输入、跨进程持久化；MCP 初始化与授权工具目录、结构化应用错误。
- 启动计划保持指令的字面值，不插入绕过运行时权限的参数。
- 替换适配器注册表后，接口 schema 和路由自动跟随；重复 ID 被拒绝，领域记录不依赖本机支持的 agent 清单。
- 前端 TypeScript 检查、Vite 生产构建、Prettier 检查。
- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --locked`：Tauri 原生桥接编译通过。
- Rust 源码格式检查通过；提交未包含测试数据、密钥或构建缓存。

未验证：真实 GitHub 数据同步、真实 agent 会话/配置投递、桌面实际渲染、Linux/Windows 实测、设备撤销与密钥恢复（尚未实现）。当前浏览器自动化未提供可用浏览器。

所有测试使用隔离数据；没有导入私人配置、修改既有项目或执行实际 agent 会话。

## 0.1.1 双入口验证

- 保留十项 Rust 核心/CLI/MCP 集成测试通过。
- `npm run test:web`：三项 Web 集成测试通过（含真实 HTTP → MCP → Rust 与生产资产提供）；涵盖创建/修改/删除、CLI 共享持久化、过期版本、三种适配器、任务材料接续、只读权限与接口目录。
- Web 拒绝错误 token、跨 Origin、Origin:null、非法 Host、静态文件路径/软链接越界、错误 JSON、超大请求与参数提权。
- TypeScript/Vite 与 Prettier 检查通过；`npm run app:build` 在 Mini 构建了 0.1.1 macOS arm64 `.app`（约 10.57 MiB）。JS API 与 Rust Tauri 均锁定为 2.11 小版本系列。最初 Tauri bundle 的签名校验失败；随后增加本地 ad-hoc 签名与严格验证，不使用个人证书。
- 使用隔离 `.local-demo` 在 Mini 启动本机 Web 服务与 App 进程；普通沙箱中的 App 进程立即退出，沙箱外启动使用同一演示目录。原生界面读取未返回有效状态，被中止；没有确认 App/Web 实际显示。浏览器工具没有可用浏览器入口。
- 41e9e04 的远端 main 与四项 CI 已只读确认全部通过；0.1.1 的 CI 结果以本次交付链接为准。

没有修改 Air、上传演示数据库、发布公网服务或导入真实个人配置。演示步骤与剩余边界见 [demo.md](demo.md)。

## 0.2.0 身份模块验证

- `cargo test --workspace`：14 项集成测试通过。新增身份测试覆盖真实能力/MCP 关联、三个适配器的字面指引、当前选择的版本检查与只读权限、同步内容与设备选择分离、失效关联和身份的阻断、并发身份冲突与合并、远端缺失依赖保留。
- `npm run test:web`：4 项测试通过，新增 Web → MCP → Rust 身份激活、CLI 读取相同当前身份、失效关联阻断与显式跳过身份流程。全部使用临时目录；网络监听仅 loopback。
- TypeScript/Vite 构建通过；Tauri 独立 manifest 检查与 macOS arm64 0.2.0 App 构建通过，开发包本地 ad-hoc 签名校验通过。
- 上一版 6f8b2ad 已按用户要求在 Air 部署 App 与本机 Web，用户确认看到了界面。本轮新身份表单未通过浏览器自动交互检查：Chrome 和 in-app browser 均报告不可用。接口测试和生产资产构建不能替代实际点击验证。

真实 GitHub 数据同步、实际 agent 进程执行、配置投递、细粒度权限和设备配对仍未完成或实测。

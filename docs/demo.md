# App / Web 双入口演示（0.2.0）

App 和浏览器 Web 共用同一份界面。Web 服务只在当前机器上运行，不是同步服务。用户已授权在 Air 查看两个入口，上一版开发包已复制到 Air 并打开，用户确认看到了界面。源码构建与打包开发包是两种运行方式；以下保留源码构建步骤。

## Air 构建

需要 Git、Rust 1.87+、Node 22.12+、macOS Xcode Command Line Tools。已有项目时在确认工作区干净后拉取；未下载时在用户选定的新目录克隆 Continuo。不要复用私人配置仓或参考仓。使用本次提交版本（由交付记录给出 commit），如下面步骤发现工具缺失，先确认安装范围。

从 Continuo 根目录：

```sh
cargo test --workspace --locked
cargo build -p continuo-cli --locked
cd apps/desktop
npm ci
npm run build
npm run test:web
npm run app:build
```

App 产物为 `apps/desktop/src-tauri/target/release/bundle/macos/Continuo.app`。构建脚本会用 `codesign --sign -` 生成并验证本地 ad-hoc 签名，不使用个人证书或 keychain 身份。这是本机生成的开发包，未完成面向公众的 Developer ID 签名、公证或安装器发布。本地构建针对当前 CPU 架构；Mini 的 arm64 包不可视为所有 Air 都兼容。

## 打开两个入口

在 `apps/desktop` 的第一个终端：

```sh
npm run demo:web
```

用 Air 的普通浏览器打开终端打印的 `http://127.0.0.1:1421`。启动终端保持运行。该演示授权本地写入和管理，未授权网络同步。

在第二个终端同一目录：

```sh
npm run demo:app
```

此命令打开真正的 `.app` 内可执行程序，并显式指定隔离数据目录。App 不需要 Web 服务；二者在该演示中共享项目根目录 `.local-demo`，CLI 也可通过 `--data-dir` 读取它。不要直接用 Finder 启动来核验共享演示数据，因为普通 App 启动默认使用 `~/.continuo`。

默认普通 Web 是 `npm run web`，只读。需要用户日常数据时，再由用户决定数据目录、权限和同步存储。

## 可演示的流程

1. 在 Web 新建身份，填写角色说明、指引和偏好 agent；在 App 刷新，读取同一条真实记录。
2. 在能力/MCP 模块登记工具，再编辑身份选择关联；检查关联，并设为本机当前身份。在 Agent 页面选择身份，生成使用指引与版本快照的真实计划。
3. 在 App 编辑记录；在 Web 刷新，验证实际持久化。
4. 查看六个核心入口与设备登记；查看三个 agent 的能力说明，生成启动参数计划。
5. 在接口控制台运行 `system.status`、`system.describe`、`entity.get` 等；通过明确任务记录调用 `session.handoff`，得到跨 agent 接续材料。
6. 以不带权限参数的 `npm run web -- --data-dir ../../.local-demo --port 1422` 启动只读入口；确认能读取、不能创建或修改，工具目录不展示未授权操作。
7. 停止 Web 服务并尝试刷新；界面应说明连接已中断。仅运行 Vite 时显示本机服务启动引导，直接打开 HTML 时保留静态启动指引。

记录最初为空，没有模拟数据或自动导入。冲突和删除传播已在临时 Git 仓库中测试；此演示不连接用户真实 GitHub 仓库。

## 当前边界

已完成身份领域管理、能力/MCP 关联校验、当前身份切换、冲突编辑合并、真实记录管理、版本校验、加密同步引擎、接口目录、启动计划、MCP 注册文档与任务材料接续。浏览器请求确实通过 HTTP → MCP → Rust 服务，App 使用同一服务的原生桥接。

尚未完成实际启动 agent、Skills/原生配置投递与回滚、原生会话发现、设备配对与密钥恢复/轮换、自动同步和安装/签名分发。跨 agent 接续不迁移内部状态。

真实 Air/Mini 跨设备同步、真实 agent 运行与原生配置写入，需要另行确认测试设备、专用存储、隔离目录和运行权限；本轮不使用真实账号、秘密或个人 agent 配置。用户已确认上一版 App/Web 的可见性。新身份界面的构建和真实接口流程已测试；浏览器工具暂不可用，尚未自动验证新表单的实际点击与渲染。

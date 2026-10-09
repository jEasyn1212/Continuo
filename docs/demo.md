# App / Web 双入口演示（0.1.1）

App 和浏览器 Web 共用同一份界面。Web 服务只在当前机器上运行，不是同步服务。Air 应在本机构建/运行，本轮 Mini 没有自动连接、安装或打开 Air；执行位置与授权另行协调。

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

App 产物为 `apps/desktop/src-tauri/target/release/bundle/macos/Continuo.app`。这是本机生成的开发包，未完成面向公众的 Developer ID 签名、公证或安装器发布。本地构建针对当前 CPU 架构；Mini 的 arm64 包不可视为所有 Air 都兼容。

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

1. 在 Web 新建一条身份或任务；在 App 刷新，读取同一条真实记录。
2. 在 App 编辑记录；在 Web 刷新，验证实际持久化。
3. 查看六个核心入口与设备登记；查看三个 agent 的能力说明，生成启动参数计划。
4. 在接口控制台运行 `system.status`、`system.describe`、`entity.get` 等；通过明确任务记录调用 `session.handoff`，得到跨 agent 接续材料。
5. 以不带权限参数的 `npm run web -- --data-dir ../../.local-demo --port 1422` 启动只读入口；确认能读取、不能创建或修改，工具目录不展示未授权操作。
6. 停止 Web 服务并尝试刷新；界面应说明连接已中断。仅运行 Vite 时显示本机服务启动引导，直接打开 HTML 时保留静态启动指引。

记录最初为空，没有模拟数据或自动导入。冲突和删除传播已在临时 Git 仓库中测试；此演示不连接用户真实 GitHub 仓库。

## 当前边界

已完成真实记录管理、版本校验、冲突选择、加密同步引擎、接口目录、启动计划、MCP 注册文档与任务材料接续。浏览器请求确实通过 HTTP → MCP → Rust 服务，App 使用同一服务的原生桥接。

尚未完成实际启动 agent、Skills/原生配置投递与回滚、原生会话发现、设备配对与密钥恢复/轮换、自动同步和安装/签名分发。跨 agent 接续不迁移内部状态。

真实 Air/Mini 跨设备同步、真实 agent 运行与原生配置写入，需要另行确认测试设备、专用存储、隔离目录和运行权限；本轮不使用真实账号、秘密或个人 agent 配置。App/Web 实际渲染与上述手动界面步骤尚待验证，构建成功及接口测试不代表这些步骤已通过。

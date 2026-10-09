import React from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import path from "node:path";
import { spawnSync } from "node:child_process";

import { call, connect, Adapter, Entity, Event as StoredEvent } from "./api";
import { SyncWorkspace } from "./SyncWorkspace";
import { SessionWorkspace } from "./SessionWorkspace";
import { McpWorkspace } from "./McpWorkspace";
import { CapabilityWorkspace } from "./CapabilityWorkspace";
import { TaskWorkspace } from "./TaskWorkspace";
import { IdentityWorkspace } from "./IdentityWorkspace";
const { startWebServer } = createRequire(path.resolve("package.json"))(
  "./web/server.mjs",
) as {
  startWebServer(
    options: Record<string, unknown>,
  ): Promise<{ url: string; close(): Promise<void> }>;
};
const binary = path.resolve("../../target/debug/continuo");
const realFetch = globalThis.fetch;
let directory: string,
  dataDir: string,
  service: { url: string; close: () => Promise<void> },
  adapters: Adapter[];
beforeEach(async () => {
  directory = await mkdtemp(path.join(tmpdir(), "continuo-task-ui-"));
  dataDir = path.join(directory, "vault");
  const assets = path.join(directory, "assets");
  await mkdir(assets);
  await writeFile(
    path.join(assets, "index.html"),
    "<!doctype html><title>Isolated task UI test</title>",
  );
  service = await startWebServer({
    binary,
    assets,
    dataDir,
    port: 0,
    writes: true,
  });
  vi.stubGlobal("fetch", ((input: RequestInfo | URL, init?: RequestInit) =>
    realFetch(
      typeof input === "string" && input.startsWith("/")
        ? new URL(input, service.url)
        : input,
      init,
    )) as typeof fetch);
  await connect();
  adapters = (await call<{ adapters: Adapter[] }>("agent.list")).adapters;
});
afterEach(async () => {
  cleanup();
  await service?.close();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  await rm(directory, { recursive: true, force: true });
});
function ui(writable = true) {
  render(
    <TaskWorkspace
      writable={writable}
      adapters={adapters}
      refreshSignal={0}
      onChange={async () => {}}
    />,
  );
}
async function fill(label: string, value: string) {
  await waitFor(() => {
    const input = screen.getByLabelText(label);
    expect(input.matches(":disabled")).toBe(false);
    fireEvent.change(input, { target: { value } });
  });
}
async function click(name: string | RegExp) {
  await waitFor(() => {
    const button = screen.getByRole("button", { name });
    expect(button.matches(":disabled")).toBe(false);
    fireEvent.click(button);
  });
}
async function createTask() {
  ui();
  await click("＋ 新建任务");
  await fill("任务名称", "Release review");
  await fill("任务目标", "Review the release and its evidence");
  await fill("验收标准", "All checks pass\nReview notes exist");
  await fill("下一步", "Read source\nVerify checks");
  await click("保存任务");
  await screen.findByRole("button", { name: "编辑目标" });
  await screen.findByRole("button", { name: "进展" });
}
test("Task forms operate the real Web/MCP core through create, status, progress, decision, artifact and handoff", async () => {
  await createTask();
  await fill("新状态", "active");
  await fill("变化原因", "Start review");
  await click("记录状态变化");
  await waitFor(() =>
    expect(
      (screen.getByLabelText("变化原因") as HTMLTextAreaElement).value,
    ).toBe(""),
  );
  await click("进展");
  await fill("本次进展", "Read all changed files");
  await fill("已执行的检查", "cargo test: passed");
  await click("记录进展");
  await screen.findByText("Read all changed files");
  await click("决策");
  await fill("决策", "Preserve both devices' edits");
  await fill("理由与权衡", "Causal history prevents lost updates");
  await click("记录决策");
  await screen.findByText("Causal history prevents lost updates");
  await click("产物");
  await fill("产物名称", "Review notes");
  await fill("可携带的引用", "project:docs/review.md");
  await fill("验证说明", "Read locally; verify again on the next device");
  await click("登记产物");
  await screen.findByText("project:docs/review.md");
  await click("接续");
  await fill("目标 agent", "codex");
  await click("生成接续材料");
  await screen.findByLabelText("接续指令");
  const prompt = (screen.getByLabelText("接续指令") as HTMLTextAreaElement)
    .value;
  expect(prompt).toContain("Read all changed files");
  expect(prompt).toContain("Causal history prevents lost updates");
  expect(prompt).toContain("project:docs/review.md");
  await fill("目标设备工作目录", "/tmp");
  await click("生成使用此任务的启动计划");
  const plan = await screen.findByLabelText("任务启动计划");
  expect(plan.textContent).toContain('"executed": false');
  const persisted = await call<{ entities: Entity[] }>("entity.list", {
    kind: "task",
  });
  const task = persisted.entities[0];
  expect(task.heads[0].data.artifacts).toHaveLength(1);
  expect(task.heads[0].data.progress).toHaveLength(2);
  await click("目标与状态");
  await fill("新状态", "done");
  await fill("完成结论", "All evidence checked");
  await click("记录状态变化");
  await screen.findByText("All evidence checked");
  await click("接续");
  expect(
    (screen.getByRole("button", { name: "生成接续材料" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await screen.findByText("先重新打开已结束的任务");
});
test("Stale UI saves show a conflict and retain the user's draft instead of replacing newer records", async () => {
  await createTask();
  await click("编辑目标");
  await fill("任务目标", "My unsaved goal");
  const all = await call<{ entities: Entity[] }>("entity.list", {
    kind: "task",
  });
  const task = all.entities[0];
  const updated = spawnSync(
    binary,
    ["--data-dir", dataDir, "call", "entity.update", "--input", "-"],
    {
      encoding: "utf8",
      input: JSON.stringify({
        id: task.id,
        expected_revision: task.heads[0].revision,
        data: { ...task.heads[0].data, goal: "Updated by another interface" },
      }),
    },
  );
  expect(updated.status).toBe(0);
  await click("保存任务");
  await screen.findByRole("alert");
  expect((screen.getByLabelText("任务目标") as HTMLTextAreaElement).value).toBe(
    "My unsaved goal",
  );
  const current = await call<Entity>("entity.get", { id: task.id });
  expect(current.heads[0].data.goal).toBe("Updated by another interface");
});
test("Read-only task UI disables record mutations while still allowing inspection and handoff", async () => {
  const task = await call<Entity>("entity.create", {
    kind: "task",
    name: "Readonly review",
    data: { goal: "Inspect only", next_steps: ["Read evidence"] },
  });
  await service.close();
  service = await startWebServer({
    binary,
    assets: path.join(directory, "assets"),
    dataDir,
    port: 0,
    writes: false,
  });
  await connect();
  ui(false);
  fireEvent.click(
    await screen.findByRole("button", { name: /^Readonly review/ }),
  );
  await screen.findByRole("button", { name: "编辑目标" });
  expect(
    (screen.getByRole("button", { name: "编辑目标" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(
    (
      (await screen.findByRole("button", {
        name: "记录状态变化",
      })) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  await click("接续");
  await click("生成接续材料");
  await screen.findByLabelText("接续指令");
  const current = await call<Entity>("entity.get", { id: task.id });
  expect(current.heads[0].revision).toBe(task.heads[0].revision);
});

test("Unsubmitted progress remains across task views and protects task changes", async () => {
  await createTask();
  const second = await call<Entity>("entity.create", {
    kind: "task",
    name: "Second task",
    data: { goal: "Another goal", next_steps: ["Another step"] },
  });
  await click("进展");
  await fill("本次进展", "Unsaved investigation");
  await fill("已执行的检查", "Check still being recorded");
  await click("决策");
  await fill("决策", "Draft decision");
  await fill("理由与权衡", "Draft rationale");
  await click("进展");
  expect((screen.getByLabelText("本次进展") as HTMLTextAreaElement).value).toBe(
    "Unsaved investigation",
  );
  const event = new Event("beforeunload", { cancelable: true });
  window.dispatchEvent(event);
  expect(event.defaultPrevented).toBe(true);
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
  await click("＋ 新建任务");
  expect(confirm).toHaveBeenCalled();
  expect(screen.queryByLabelText("任务名称")).toBeNull();
  expect(
    (await call<Entity>("entity.get", { id: second.id })).heads[0].data.goal,
  ).toBe("Another goal");
});
test("Identity forms keep their real capability bindings and device-local selection after task UI changes", async () => {
  const cap = await call<Entity>("entity.create", {
    kind: "capability",
    name: "Source rules",
    data: { description: "Check citations" },
  });
  const mcp = await call<Entity>("entity.create", {
    kind: "mcp",
    name: "Local tools",
    data: {},
  });
  render(
    <IdentityWorkspace
      writable
      adapters={adapters}
      refreshSignal={0}
      onChange={async () => {}}
    />,
  );
  await click("＋ 新建身份");
  await fill("身份名称", "Research identity");
  await fill("角色说明", "Research work");
  await fill("给 agent 的身份指引", "Verify claims before reporting");
  await fill("偏好 agent", "codex");
  fireEvent.click(
    await screen.findByRole("checkbox", { name: /Source rules/ }),
  );
  fireEvent.click(screen.getByRole("checkbox", { name: /Local tools/ }));
  await click("保存身份");
  await screen.findByRole("button", { name: "编辑身份" });
  await click("设为本机当前身份");
  await waitFor(async () => {
    const current = await call<{
      state: string;
      context: { identity: StoredEvent };
    }>("identity.current");
    expect(current.state).toBe("ready");
    expect(current.context.identity.name).toBe("Research identity");
  });
  const all = await call<{ entities: Entity[] }>("entity.list", {
    kind: "identity",
  });
  expect(all.entities[0].heads[0].data.capability_ids).toEqual([cap.id]);
  expect(all.entities[0].heads[0].data.mcp_ids).toEqual([mcp.id]);
  await click("检查关联");
  await screen.findByText("指引与关联记录有效");
  await click("清除当前身份");
  await waitFor(async () =>
    expect((await call<{ state: string }>("identity.current")).state).toBe(
      "none",
    ),
  );
});

test("Capability forms review exact text and produce real adapter plans; editing invalidates review", async () => {
  render(
    <CapabilityWorkspace
      writable
      adapters={adapters}
      refreshSignal={0}
      onChange={async () => {}}
    />,
  );
  await click("导入文本");
  await fill("能力名称", "Source checking");
  await fill("内容版本", "1.0");
  await fill("能力正文", "Literal $(do-not-run)\nCheck primary evidence.");
  await fill("来源引用", "project:skills/research/SKILL.md");
  await fill("来源版本或 commit", "fixture-revision");
  await fill("来源许可", "MIT");
  await click("保存能力");
  await screen.findByText("正文或元信息尚待检查 · Source checking");
  expect(
    screen
      .getByRole("button", { name: "生成能力启动计划" })
      .matches(":disabled"),
  ).toBe(true);
  await click("标记正文已检查");
  await screen.findByText("当前正文与依赖已检查，可加入启动计划。");
  await fill("本机工作目录", "/tmp");
  await click("生成能力启动计划");
  await screen.findByText(/"capability_context":/);
  expect(screen.getByText(/"capability_context":/).textContent).toContain(
    '"permissions_granted": false',
  );
  const stored = (
    await call<{ entities: Entity[] }>("entity.list", { kind: "capability" })
  ).entities[0];
  expect(stored.heads[0].data.source_revision).toBe("fixture-revision");
  await click("编辑能力");
  await fill("能力正文", "Changed evidence policy");
  await click("保存能力");
  await screen.findByText("正文或元信息尚待检查 · Source checking");
  expect(
    screen
      .getByRole("button", { name: "生成能力启动计划" })
      .matches(":disabled"),
  ).toBe(true);
  expect(screen.queryByText(/"capability_context":/)).toBeNull();
});

test("Capability stale saves preserve drafts; cancel restores inspection and read-only interface cannot review", async () => {
  const cap = await call<Entity>("entity.create", {
    kind: "capability",
    name: "Draft rule",
    data: { body: "Base rule" },
  });
  render(
    <CapabilityWorkspace
      writable
      adapters={adapters}
      refreshSignal={0}
      onChange={async () => {}}
    />,
  );
  await click(/Draft rule/);
  await screen.findByRole("button", { name: "标记正文已检查" });
  await click("编辑能力");
  await fill("能力正文", "My unsaved content");
  const result = spawnSync(
    binary,
    ["--data-dir", dataDir, "call", "entity.update", "--input", "-"],
    {
      encoding: "utf8",
      input: JSON.stringify({
        id: cap.id,
        expected_revision: cap.heads[0].revision,
        data: { body: "External content" },
      }),
    },
  );
  expect(result.status).toBe(0);
  await click("保存能力");
  await screen.findByRole("alert");
  expect((screen.getByLabelText("能力正文") as HTMLTextAreaElement).value).toBe(
    "My unsaved content",
  );
  vi.spyOn(window, "confirm").mockReturnValue(true);
  await click("取消编辑");
  await screen.findByRole("button", { name: "标记正文已检查" });
  cleanup();
  await service.close();
  service = await startWebServer({
    binary,
    assets: path.join(directory, "assets"),
    dataDir,
    port: 0,
    writes: false,
  });
  await connect();
  render(
    <CapabilityWorkspace
      writable={false}
      adapters={adapters}
      refreshSignal={0}
      onChange={async () => {}}
    />,
  );
  await click(/Draft rule/);
  await screen.findByText("External content");
  await screen.findByRole("button", { name: "标记正文已检查" });
  expect(
    screen.getByRole("button", { name: "编辑能力" }).matches(":disabled"),
  ).toBe(true);
  expect(
    screen.getByRole("button", { name: "标记正文已检查" }).matches(":disabled"),
  ).toBe(true);
  await expect(
    call("capability.review", {
      id: cap.id,
      expected_revision: cap.heads[0].revision,
      expected_digest: "wrong",
    }),
  ).rejects.toThrow("未获授权");
});

test("Capability compatibility is checked in forms against the registered adapters", async () => {
  const cap = await call<Entity>("entity.create", {
    kind: "capability",
    name: "Codex rule",
    data: { body: "Codex only", agent_targets: ["codex"] },
  });
  const inspection = await call<{ digest: string }>("capability.inspect", {
    id: cap.id,
  });
  await call("capability.review", {
    id: cap.id,
    expected_revision: cap.heads[0].revision,
    expected_digest: inspection.digest,
  });
  render(
    <CapabilityWorkspace
      writable
      adapters={adapters}
      refreshSignal={0}
      onChange={async () => {}}
    />,
  );
  await click(/Codex rule/);
  await screen.findByText("不适用于当前 agent · Codex rule");
  expect(
    screen
      .getByRole("button", { name: "生成能力启动计划" })
      .matches(":disabled"),
  ).toBe(true);
  await fill("检查目标 agent", "codex");
  await screen.findByText("当前正文与依赖已检查，可加入启动计划。");
});

async function enableIsolatedProbes() {
  await service.close();
  service = await startWebServer({
    binary,
    assets: path.join(directory, "assets"),
    dataDir,
    port: 0,
    writes: true,
    admin: true,
    probes: true,
  });
  await connect();
}
test("MCP forms separate synced definitions, local maps, manual checks and registration plans against the real core", async () => {
  await enableIsolatedProbes();
  render(
    <McpWorkspace
      writable
      admin
      probes
      adapters={adapters}
      refreshSignal={0}
      onChange={async () => {}}
    />,
  );
  await click("＋ 新建 MCP");
  await fill("MCP 名称", "Local fixture tools");
  await fill("注册名称", "fixture-tools");
  await fill("程序提示", "continuo");
  await click("保存 MCP 定义");
  await click("配置本机映射");
  await fill("可执行文件", binary);
  await fill(
    "命令参数（JSON 数组）",
    JSON.stringify([
      "--data-dir",
      path.join(directory, "isolated-mcp-server"),
      "mcp",
    ]),
  );
  await click("保存本机映射");
  await click("生成 MCP 注册计划");
  await screen.findByText(/"config_written": false/);
  expect(
    (screen.getByLabelText("可复制的注册文档") as HTMLTextAreaElement).value,
  ).toContain("fixture-tools");
  const confirmation = vi.spyOn(window, "confirm").mockReturnValue(false);
  await click("检查连接（需确认）");
  expect(confirmation.mock.calls[0][0]).toContain(binary);
  const all = (
    await call<{ entities: Entity[] }>("entity.list", { kind: "mcp" })
  ).entities;
  expect(
    (
      await call<{ connection: { state: string } }>("mcp.inspect", {
        id: all[0].id,
        target_agent: "codex",
      })
    ).connection.state,
  ).toBe("not_checked");
  confirmation.mockReturnValue(true);
  await click("检查连接（需确认）");
  await screen.findByText("检查成功");
  await click("清除本机映射");
  await screen.findByText("这台设备尚未配置运行命令");
  expect(
    screen
      .getByRole("button", { name: "检查连接（需确认）" })
      .matches(":disabled"),
  ).toBe(true);
});

test("MCP connection check cancellation remains available while the protocol request is pending", async () => {
  await enableIsolatedProbes();
  const file = path.join(directory, "isolated-hanging-server.mjs");
  await writeFile(file, "process.stdin.resume();\n");
  const record = await call<Entity>("entity.create", {
    kind: "mcp",
    name: "Hanging fixture",
    data: { server_key: "hanging-fixture" },
  });
  await call("mcp.map", {
    id: record.id,
    expected_revision: record.heads[0].revision,
    expected_mapping_revision: "none",
    mapping: { executable: process.execPath, args: [file] },
  });
  vi.spyOn(window, "confirm").mockReturnValue(true);
  render(
    <McpWorkspace
      writable
      admin
      probes
      adapters={adapters}
      refreshSignal={0}
      onChange={async () => {}}
    />,
  );
  await click(/Hanging fixture/);
  await click("检查连接（需确认）");
  await screen.findByRole("button", { name: "取消连接检查" });
  await waitFor(async () =>
    expect(
      (
        await call<{ connection: { state: string } }>("mcp.inspect", {
          id: record.id,
          target_agent: "codex",
        })
      ).connection.state,
    ).toBe("running"),
  );
  await click("取消连接检查");
  await screen.findByText("已取消");
  const inspect = await call<{ connection: { state: string } }>("mcp.inspect", {
    id: record.id,
    target_agent: "codex",
  });
  expect(inspect.connection.state).toBe("cancelled");
});

async function sessionFixture() {
  const task = await call<Entity>("entity.create", {
    kind: "task",
    name: "Linked task",
    data: { goal: "Continue recorded work", next_steps: ["Check source"] },
  });
  return {
    task,
    session: await call<Entity>("session.register", {
      name: "Recorded session",
      data: {
        agent: "codex",
        native_session_id: "fixture-native-id",
        task_id: task.id,
        summary: "Review reached verification",
      },
    }),
  };
}
function sessionUi(writable = true, refreshSignal: unknown = 0) {
  return (
    <SessionWorkspace
      writable={writable}
      admin={true}
      adapters={adapters}
      refreshSignal={refreshSignal}
      onChange={async () => {}}
    />
  );
}
test("Session UI registers links, generates fresh handoff context and records cancellation without process execution", async () => {
  const task = await call<Entity>("entity.create", {
    kind: "task",
    name: "Linked task",
    data: { goal: "Review change", next_steps: ["Read tests"] },
  });
  render(sessionUi());
  await click("＋ 登记会话");
  await fill("会话名称", "New recorded session");
  await fill("来源 agent", "codex");
  await fill("原生会话 ID", "fixture-native-id");
  await fill("关联任务", task.id);
  await fill("可携带的会话摘要", "Reviewed source, verify tests next");
  await click("保存会话");
  await screen.findByRole("button", { name: "编辑会话关联" });
  await click("生成会话接续材料");
  const packet = await screen.findByLabelText("会话接续指令");
  expect((packet as HTMLTextAreaElement).value).toContain("Reviewed source");
  expect((packet as HTMLTextAreaElement).value).not.toContain(
    "fixture-native-id",
  );
  await fill("接续设备工作目录", dataDir);
  await fill("接续目标 agent", "hermes");
  await click("生成新会话接续计划");
  expect((await screen.findByLabelText("会话计划")).textContent).toContain(
    '"executed": false',
  );
  expect(screen.getByLabelText("会话计划").textContent).not.toContain(
    "fixture-native-id",
  );
  await fill("会话新状态", "cancelled");
  await fill("会话变化原因", "Stop this recorded work");
  await click("记录会话状态");
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: "生成会话接续材料" })
        .matches(":disabled"),
    ).toBe(true),
  );
});
test("Session environment is local, needs explicit native-state confirmation, and supports discard and clearing", async () => {
  await service.close();
  service = await startWebServer({
    binary,
    assets: path.join(directory, "assets"),
    dataDir,
    port: 0,
    writes: true,
    admin: true,
  });
  await connect();
  const { session } = await sessionFixture();
  render(sessionUi());
  await click(/Recorded session/);
  await screen.findByRole("button", { name: "编辑会话关联" });
  await click("配置本机环境");
  await fill("本机 agent 可执行路径", binary);
  await fill("本机工作目录", dataDir);
  await fill("本机账号引用", "credential:test-reference");
  await click("保存本机环境");
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: "生成原生恢复计划" })
        .matches(":disabled"),
    ).toBe(true),
  );
  await click("配置本机环境");
  const confirm = screen.getByLabelText(
    "我已确认这台设备和所用账号下存在这个原生会话",
  );
  fireEvent.click(confirm);
  await click("保存本机环境");
  await click("生成原生恢复计划");
  expect((await screen.findByLabelText("会话计划")).textContent).toContain(
    '"mode": "native_resume"',
  );
  expect(screen.getByLabelText("会话计划").textContent).toContain(
    '"account_authentication_verified": false',
  );
  await click("编辑会话关联");
  await fill("会话名称", "Unsaved rename");
  vi.spyOn(window, "confirm").mockReturnValue(false);
  await click("取消编辑");
  expect((screen.getByLabelText("会话名称") as HTMLInputElement).value).toBe(
    "Unsaved rename",
  );
  vi.mocked(window.confirm).mockReturnValue(true);
  await click("取消编辑");
  expect(
    (await call<Entity>("entity.get", { id: session.id })).heads[0].name,
  ).toBe("Recorded session");
  await click("清除本机环境");
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: "生成原生恢复计划" })
        .matches(":disabled"),
    ).toBe(true),
  );
});
test("Session stale edits retain draft and readonly UI prevents mutations", async () => {
  const { session } = await sessionFixture();
  const view = render(sessionUi());
  await click(/Recorded session/);
  await click("编辑会话关联");
  await fill("会话名称", "Draft preserved");
  await call("entity.update", {
    id: session.id,
    expected_revision: session.heads[0].revision,
    name: "Other entry update",
  });
  await click("保存会话");
  await screen.findByRole("alert");
  expect((screen.getByLabelText("会话名称") as HTMLInputElement).value).toBe(
    "Draft preserved",
  );
  view.unmount();
  render(sessionUi(false));
  expect(
    screen.getByRole("button", { name: "＋ 登记会话" }).matches(":disabled"),
  ).toBe(true);
  await click(/Other entry update/);
  await screen.findByRole("button", { name: "编辑会话关联" });
  expect(
    screen.getByRole("button", { name: "编辑会话关联" }).matches(":disabled"),
  ).toBe(true);
  expect(
    screen.getByRole("button", { name: "配置本机环境" }).matches(":disabled"),
  ).toBe(true);
});

test("Session checks refresh when linked task changes and remove plans from the old context", async () => {
  const { session, task } = await sessionFixture();
  const view = render(sessionUi());
  await click(/Recorded session/);
  await click("生成会话接续材料");
  await screen.findByLabelText("会话接续指令");
  await call("entity.delete", {
    id: task.id,
    expected_revision: task.heads[0].revision,
  });
  view.rerender(sessionUi(true, 1));
  await screen.findByText("关联任务缺失、已删除或有冲突");
  expect(screen.queryByLabelText("会话接续指令")).toBeNull();
  expect(
    screen
      .getByRole("button", { name: "生成会话接续材料" })
      .matches(":disabled"),
  ).toBe(true);
  expect(
    (await call<Entity>("entity.get", { id: session.id })).heads[0].revision,
  ).toBe(session.heads[0].revision);
});

async function syncUi(allowed = true) {
  await service.close();
  service = await startWebServer({
    binary,
    assets: path.join(directory, "assets"),
    dataDir,
    port: 0,
    writes: true,
    admin: true,
    sync: allowed,
  });
  await connect();
  render(
    <SyncWorkspace
      writable={true}
      admin={true}
      networkAllowed={allowed}
      refreshSignal={0}
      onChange={async () => {}}
      onOpenModule={() => {}}
    />,
  );
}
function fixtureGit(args: string[]) {
  const result = spawnSync("git", args, { encoding: "utf8" });
  expect(result.status).toBe(0);
}
function secondCall(vault: string, method: string, params: object = {}) {
  const result = spawnSync(
    binary,
    ["--data-dir", vault, "--allow-sync", "call", method, "--input", "-"],
    { encoding: "utf8", input: JSON.stringify(params) },
  );
  expect(result.status).toBe(0);
  return JSON.parse(result.stdout).data;
}
async function configureSyncForm() {
  const remote = path.join(directory, "storage.git"),
    key = path.join(directory, "key");
  fixtureGit(["init", "--bare", "--quiet", remote]);
  await click("配置同步存储");
  await fill("用户存储地址", remote);
  await fill("本机工作区密钥文件", key);
  await click("为新工作区生成密钥");
  await click("保存同步配置");
  await waitFor(() =>
    expect(screen.queryByLabelText("用户存储地址")).toBeNull(),
  );
  return { remote, key };
}
test("Sync forms configure a real encrypted fixture, preview pending records, synchronize, pause and safely retry", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  await call("entity.create", {
    kind: "task",
    name: "Real portable record",
    data: { goal: "Check" },
  });
  await syncUi();
  const { remote } = await configureSyncForm();
  await click("检查远端待同步");
  await waitFor(async () =>
    expect((await call<any>("sync.inspect")).pending_upload).toBe(1),
  );
  const branches = spawnSync("git", ["--git-dir", remote, "branch", "--list"], {
    encoding: "utf8",
  });
  expect(branches.stdout.trim()).toBe("");
  await click("同步 / 安全重试");
  await waitFor(async () =>
    expect((await call<any>("sync.inspect")).pending_upload).toBe(0),
  );
  await click("停用本机同步");
  await screen.findByRole("button", { name: "恢复本机同步" });
  expect(
    (
      screen.getByRole("button", {
        name: "同步 / 安全重试",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  expect((await call<any>("sync.inspect")).device_revocation_supported).toBe(
    false,
  );
  await click("恢复本机同步");
  await click("同步 / 安全重试");
  await waitFor(async () =>
    expect((await call<any>("sync.inspect")).job.result.uploaded).toBe(0),
  );
}, 20000);
test("Sync recovery previews explicit history, cancellation makes no write, and restore preserves the tombstone", async () => {
  const record = await call<Entity>("entity.create", {
    kind: "task",
    name: "Restore work",
    data: { goal: "Kept goal" },
  });
  const tombstone = await call<Entity>("entity.delete", {
    id: record.id,
    expected_revision: record.heads[0].revision,
  });
  await syncUi(false);
  await fill("已删除的记录", record.id);
  await fill("要恢复的历史 revision", record.heads[0].revision);
  await click("查看历史版本");
  await screen.findByLabelText("恢复版本预览");
  await click("取消恢复");
  expect(
    (await call<Entity>("entity.get", { id: record.id })).heads[0].deleted,
  ).toBe(true);
  await fill("已删除的记录", record.id);
  await fill("要恢复的历史 revision", record.heads[0].revision);
  await click("查看历史版本");
  await click("恢复所选版本");
  await waitFor(async () =>
    expect(
      (await call<Entity>("entity.get", { id: record.id })).heads[0].deleted,
    ).toBe(false),
  );
  const restored = await call<Entity>("entity.get", { id: record.id });
  expect(restored.heads[0].parents).toEqual([tombstone.heads[0].revision]);
  expect(
    (await call<any>("entity.history", { id: record.id })).total_versions,
  ).toBe(3);
  expect(
    (
      screen.getByRole("button", {
        name: "同步 / 安全重试",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
});
test("Sync conflicts keep both real encrypted device versions until an explicit merge references both", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  await syncUi();
  const { remote, key } = await configureSyncForm();
  const a = await call<Entity>("entity.create", {
    kind: "task",
    name: "Concurrent work",
    data: { goal: "Initial" },
  });
  await call("sync.run");
  const other = path.join(directory, "other");
  secondCall(other, "sync.configure", { remote, key_file: key });
  secondCall(other, "sync.run");
  await call("entity.update", {
    id: a.id,
    expected_revision: a.heads[0].revision,
    data: { goal: "Mini choice" },
  });
  secondCall(other, "entity.update", {
    id: a.id,
    expected_revision: a.heads[0].revision,
    data: { goal: "Air choice" },
  });
  secondCall(other, "sync.run");
  await call("sync.run");
  await click("刷新同步状态");
  await click("核对并整理版本");
  const conflicted = await call<Entity>("entity.get", { id: a.id });
  expect(conflicted.heads).toHaveLength(2);
  await fill("整理后的记录内容", JSON.stringify({ goal: "Keep both choices" }));
  await click("保存冲突整理");
  await waitFor(async () =>
    expect((await call<Entity>("entity.get", { id: a.id })).conflicted).toBe(
      false,
    ),
  );
  const merged = await call<Entity>("entity.get", { id: a.id });
  expect(merged.heads[0].parents.sort()).toEqual(
    conflicted.heads.map((h) => h.revision).sort(),
  );
  expect(merged.heads[0].data.goal).toBe("Keep both choices");
}, 20000);

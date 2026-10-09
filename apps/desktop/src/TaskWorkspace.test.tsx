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
async function click(name: string) {
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
  await screen.findByText("指引与关联已就绪");
  await click("清除当前身份");
  await waitFor(async () =>
    expect((await call<{ state: string }>("identity.current")).state).toBe(
      "none",
    ),
  );
});

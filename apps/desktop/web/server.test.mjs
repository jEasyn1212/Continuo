import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, mkdir, symlink, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import http from "node:http";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { startWebServer } from "./server.mjs";

const binary = fileURLToPath(
  new URL("../../../target/debug/continuo", import.meta.url),
);
async function fixture(t, permissions) {
  const dir = await mkdtemp(path.join(tmpdir(), "continuo-web-"));
  const assets = path.join(dir, "assets");
  await mkdir(assets);
  await writeFile(
    path.join(assets, "index.html"),
    "<!doctype html><title>Continuo</title>",
  );
  await writeFile(path.join(dir, "outside.js"), "PRIVATE TEST FILE");
  await symlink(path.join(dir, "outside.js"), path.join(assets, "outside.js"));
  const dataDir = path.join(dir, "vault");
  const server = await startWebServer({
    binary,
    assets,
    dataDir,
    port: 0,
    ...permissions,
  });
  t.after(async () => {
    await server.close();
    await rm(dir, { recursive: true, force: true });
  });
  const bootstrap = await fetch(`${server.url}/api/bootstrap`, {
    headers: { "X-Continuo-Client": "web-v1" },
  });
  const { data } = await bootstrap.json();
  const headers = {
    "Content-Type": "application/json",
    "X-Continuo-Client": "web-v1",
    Authorization: `Bearer ${data.token}`,
    Origin: server.url,
  };
  const call = async (method, params = {}, extraHeaders = {}) => {
    const response = await fetch(`${server.url}/api/call`, {
      method: "POST",
      headers: { ...headers, ...extraHeaders },
      body: JSON.stringify({ method, params }),
    });
    return { status: response.status, body: await response.json() };
  };
  return { ...server, data, dataDir, call, headers };
}

test("Web uses real core records, CLI shares data, and optimistic writes remain enforced", async (t) => {
  const f = await fixture(t, { writes: true, admin: true });
  const html = await fetch(f.url);
  assert.match(await html.text(), /Continuo/);
  assert.match(
    html.headers.get("content-security-policy"),
    /frame-ancestors 'none'/,
  );
  assert.deepEqual(f.data.permissions, {
    writes: true,
    admin: true,
    sync: false,
  });
  const created = (
    await f.call("entity.create", {
      kind: "task",
      name: "Web actual record",
      data: { next_steps: ["Continue via CLI"] },
    })
  ).body;
  assert.equal(created.ok, true);
  const { id, heads } = created.data;
  const cli = spawnSync(
    binary,
    ["--data-dir", f.dataDir, "call", "entity.get", "--input", "-"],
    { input: JSON.stringify({ id }), encoding: "utf8" },
  );
  assert.equal(cli.status, 0);
  assert.equal(JSON.parse(cli.stdout).data.heads[0].name, "Web actual record");
  const next = (
    await f.call("entity.update", {
      id,
      expected_revision: heads[0].revision,
      name: "Updated in Web",
    })
  ).body;
  assert.equal(next.ok, true);
  assert.equal(
    (
      await f.call("entity.update", {
        id,
        expected_revision: heads[0].revision,
        name: "Stale",
      })
    ).body.error.code,
    "revision_conflict",
  );
  assert.equal((await f.call("sync.run")).body.error.code, "permission_denied");
  assert.deepEqual(
    (await f.call("agent.list")).body.data.adapters.map((a) => a.id),
    ["claude-code", "codex", "hermes"],
  );
  assert.equal(
    (await f.call("session.handoff", { task_id: id, target_agent: "codex" }))
      .body.data.internal_state_transferred,
    false,
  );
  const catalog = (await f.call("system.describe")).body.data.operations;
  assert.ok(catalog.some((op) => op.method === "entity.delete"));
  assert.ok(!catalog.some((op) => op.method === "sync.run"));
  assert.equal(
    (
      await f.call("entity.delete", {
        id,
        expected_revision: next.data.heads[0].revision,
      })
    ).body.ok,
    true,
  );
  assert.equal((await f.call("entity.list")).body.data.entities.length, 0);
});

test("Web denies unauthorized origins, Host rebinding, token omission, unsafe files and privilege escalation", async (t) => {
  const f = await fixture(t, {});
  assert.equal(
    f.data.tools.some((tool) => tool.name === "continuo_entity_create"),
    false,
  );
  assert.equal(
    (await f.call("entity.create", { kind: "task", name: "Denied", data: {} }))
      .body.error.code,
    "permission_denied",
  );
  assert.equal(
    (await f.call("system.status", {}, { Authorization: "Bearer wrong" }))
      .status,
    401,
  );
  assert.equal(
    (await f.call("system.status", {}, { Origin: "https://untrusted.example" }))
      .status,
    403,
  );
  assert.equal((await fetch(`${f.url}/api/bootstrap`)).status, 403);
  assert.equal(
    (
      await fetch(`${f.url}/api/bootstrap`, {
        headers: { "X-Continuo-Client": "web-v1", Origin: "null" },
      })
    ).status,
    403,
  );
  assert.equal((await fetch(`${f.url}/outside.js`)).status, 404);
  assert.equal((await fetch(`${f.url}/%2e%2e%2foutside.js`)).status, 404);
  const badHost = await new Promise((resolve, reject) => {
    const req = http.get(
      f.url,
      { headers: { Host: "attacker.example" } },
      (response) => {
        response.resume();
        resolve(response.statusCode);
      },
    );
    req.on("error", reject);
  });
  assert.equal(badHost, 403);
  const invalid = await fetch(`${f.url}/api/call`, {
    method: "POST",
    headers: f.headers,
    body: "{bad",
  });
  assert.equal(invalid.status, 400);
  const oversized = await fetch(`${f.url}/api/call`, {
    method: "POST",
    headers: f.headers,
    body: "x".repeat(1024 * 1024 + 1),
  });
  assert.equal(oversized.status, 413);
  assert.equal(
    (await f.call("system.status", { allow_writes: true })).body.error.code,
    "invalid_params",
  );
  assert.equal((await f.call("system.status")).body.ok, true);
  assert.equal((await f.call("entity.list")).body.data.entities.length, 0);
});

test("production HTML and its compiled assets are served independently of Tauri", async (t) => {
  const dir = await mkdtemp(path.join(tmpdir(), "continuo-web-production-"));
  const server = await startWebServer({
    binary,
    dataDir: path.join(dir, "vault"),
    port: 0,
  });
  t.after(async () => {
    await server.close();
    await rm(dir, { recursive: true, force: true });
  });
  const page = await fetch(server.url);
  const html = await page.text();
  assert.equal(page.status, 200);
  assert.match(html, /Continuo · App \/ Web/);
  assert.match(html, /npm run web/);
  const urls = [...html.matchAll(/(?:src|href)="([^"]*assets[^" ]*)"/g)].map(
    (match) => new URL(match[1], server.url),
  );
  assert.ok(urls.length >= 2);
  for (const url of urls) {
    const asset = await fetch(url);
    assert.equal(asset.status, 200);
    assert.match(asset.headers.get("content-type"), /javascript|css/);
  }
  const bootstrap = await fetch(`${server.url}/api/bootstrap`, {
    headers: { "X-Continuo-Client": "web-v1" },
  });
  assert.equal((await bootstrap.json()).data.server.version, "0.2.0");
});

test("Identity operations cross Web/MCP/CLI with local selection, real bindings and fail-closed preparation", async (t) => {
  const f = await fixture(t, { writes: true });
  const cap = (
    await f.call("entity.create", {
      kind: "capability",
      name: "Source rules",
      data: {},
    })
  ).body.data;
  const profile = (
    await f.call("entity.create", {
      kind: "identity",
      name: "Research",
      data: {
        instructions: "Check evidence",
        capability_ids: [cap.id],
        preferred_agent: "codex",
      },
    })
  ).body.data;
  const inspect = (await f.call("identity.inspect", { id: profile.id })).body;
  assert.equal(inspect.data.ready, true);
  const current = (await f.call("identity.current")).body.data;
  const selected = (
    await f.call("identity.activate", {
      id: profile.id,
      expected_revision: profile.heads[0].revision,
      expected_selection_revision: current.selection.revision,
    })
  ).body;
  assert.equal(selected.ok, true);
  const cli = spawnSync(
    binary,
    ["--data-dir", f.dataDir, "call", "identity.current"],
    { encoding: "utf8" },
  );
  assert.equal(cli.status, 0, cli.stderr);
  assert.equal(
    JSON.parse(cli.stdout).data.context.identity.entity_id,
    profile.id,
  );
  const plan = (await f.call("agent.prepare", { agent: "codex", cwd: "/tmp" }))
    .body;
  assert.equal(plan.data.identity_context.identity.name, "Research");
  assert.equal(plan.data.executed, false);
  const stale = (
    await f.call("identity.clear", { expected_selection_revision: "none" })
  ).body;
  assert.equal(stale.error.code, "selection_conflict");
  await f.call("entity.delete", {
    id: cap.id,
    expected_revision: cap.heads[0].revision,
  });
  const broken = (
    await f.call("agent.prepare", { agent: "codex", cwd: "/tmp" })
  ).body;
  assert.equal(broken.error.code, "identity_bindings_unavailable");
  assert.equal(
    (await f.call("identity.current")).body.data.state,
    "unavailable",
  );
  const neutral = (
    await f.call("agent.prepare", {
      agent: "codex",
      cwd: "/tmp",
      use_current_identity: false,
    })
  ).body;
  assert.equal(neutral.data.identity_context, null);
});

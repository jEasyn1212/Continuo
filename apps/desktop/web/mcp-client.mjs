import { spawn } from "node:child_process";

// Transport only: every operation goes through the same MCP/service authorization.
export async function connectCore({
  binary,
  dataDir,
  writes,
  sync,
  admin,
  probes,
  processes,
}) {
  if ((sync || admin) && !writes)
    throw new Error("--allow-sync and --allow-admin require --allow-writes");
  if (probes && (!writes || !admin))
    throw new Error(
      "--allow-mcp-probes requires --allow-writes and --allow-admin",
    );
  if (processes && (!writes || !admin))
    throw new Error(
      "--allow-managed-processes requires --allow-writes and --allow-admin",
    );
  const args = dataDir ? ["--data-dir", dataDir, "mcp"] : ["mcp"];
  if (writes) args.push("--allow-writes");
  if (sync) args.push("--allow-sync");
  if (admin) args.push("--allow-admin");
  if (probes) args.push("--allow-mcp-probes");
  if (processes) args.push("--allow-managed-processes");
  const child = spawn(binary, args, { stdio: ["pipe", "pipe", "pipe"] });
  let nextId = 0,
    buffer = "",
    dead = false;
  const pending = new Map();
  function fail() {
    if (dead) return;
    dead = true;
    for (const { reject, timer } of pending.values()) {
      clearTimeout(timer);
      reject(
        new Error(
          "Local core stopped or timed out. Refresh before retrying a mutation.",
        ),
      );
    }
    pending.clear();
    child.kill();
  }
  child.on("error", fail);
  child.on("exit", fail);
  child.stdin.on("error", fail);
  child.stderr.resume(); // Never forward private paths or raw runtime diagnostics to HTTP.
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    if (Buffer.byteLength(buffer) > 16 * 1024 * 1024) return fail();
    let end;
    while ((end = buffer.indexOf("\n")) !== -1) {
      let message;
      try {
        message = JSON.parse(buffer.slice(0, end));
      } catch {
        return fail();
      }
      buffer = buffer.slice(end + 1);
      const request = pending.get(message.id);
      if (!request) continue;
      pending.delete(message.id);
      clearTimeout(request.timer);
      if (message.error)
        request.reject(new Error("Core rejected the protocol request"));
      else request.resolve(message.result);
    }
  });
  function request(method, params = {}) {
    if (dead) return Promise.reject(new Error("Local core is unavailable"));
    if (pending.size >= 64)
      return Promise.reject(new Error("Local core is busy"));
    const id = ++nextId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(fail, 120_000);
      pending.set(id, { resolve, reject, timer });
      child.stdin.write(
        JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n",
      );
    });
  }
  try {
    const info = await request("initialize", {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "continuo-local-web", version: "1" },
    });
    child.stdin.write(
      JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) +
        "\n",
    );
    const { tools } = await request("tools/list");
    const enabled = new Set(tools.map((tool) => tool.name));
    return {
      info,
      tools,
      health: () => request("ping"),
      async call(method, params) {
        const name = `continuo_${method.replaceAll(".", "_")}`;
        if (!enabled.has(name))
          return {
            api_version: "1",
            ok: false,
            error: {
              code: "permission_denied",
              message: "Operation is not enabled for this Web instance",
            },
          };
        const result = await request("tools/call", { name, arguments: params });
        if (
          !result.structuredContent ||
          typeof result.structuredContent.ok !== "boolean"
        )
          throw new Error("Core returned an invalid response");
        return result.structuredContent;
      },
      async close() {
        fail();
        if (child.exitCode !== null || child.signalCode !== null) return;
        await new Promise((resolve) => {
          const timer = setTimeout(() => {
            child.kill("SIGKILL");
            resolve();
          }, 2000);
          child.once("close", () => {
            clearTimeout(timer);
            resolve();
          });
        });
      },
    };
  } catch (error) {
    fail();
    throw error;
  }
}

import http from "node:http";
import { randomBytes, timingSafeEqual } from "node:crypto";
import { readFile, realpath, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { connectCore } from "./mcp-client.mjs";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const limit = 1024 * 1024;
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
};
function failure(code, message) {
  return { api_version: "1", ok: false, error: { code, message } };
}
function json(response, status, value) {
  response.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
  });
  response.end(JSON.stringify(value));
}
function matchesToken(value, expected) {
  if (typeof value !== "string") return false;
  const actual = Buffer.from(value);
  return actual.length === expected.length && timingSafeEqual(actual, expected);
}
async function body(request) {
  let size = 0;
  const chunks = [];
  for await (const chunk of request) {
    size += chunk.length;
    if (size > limit) throw new Error("input_too_large");
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

export async function startWebServer(options = {}) {
  const assets = await realpath(
    options.assets ?? path.join(root, "apps/desktop/dist"),
  );
  await stat(path.join(assets, "index.html"));
  const core = await connectCore({
    binary: options.binary ?? path.join(root, "target/debug/continuo"),
    ...options,
  });
  const token = randomBytes(32).toString("hex");
  const expectedToken = Buffer.from(`Bearer ${token}`);
  let authority;
  const server = http.createServer(async (request, response) => {
    response.setHeader("Cache-Control", "no-store");
    response.setHeader("X-Content-Type-Options", "nosniff");
    response.setHeader("Referrer-Policy", "no-referrer");
    response.setHeader(
      "Content-Security-Policy",
      "default-src 'self'; connect-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'",
    );
    try {
      // Reject DNS rebinding and cross-origin browser access. No CORS permissions.
      if (
        request.headers.host !== authority ||
        (request.headers.origin &&
          request.headers.origin !== `http://${authority}`)
      ) {
        request.resume();
        return json(
          response,
          403,
          failure("origin_denied", "Use the local URL printed by Continuo"),
        );
      }
      const url = new URL(request.url, `http://${authority}`);
      if (url.pathname.startsWith("/api/")) {
        if (request.headers["x-continuo-client"] !== "web-v1") {
          request.resume();
          return json(
            response,
            403,
            failure("client_required", "Open the local Continuo Web interface"),
          );
        }
        if (url.pathname === "/api/bootstrap" && request.method === "GET") {
          await core.health();
          return json(response, 200, {
            api_version: "1",
            ok: true,
            data: {
              token,
              mode: "web",
              server: core.info.serverInfo,
              tools: core.tools,
              permissions: {
                writes: !!options.writes,
                sync: !!options.sync,
                admin: !!options.admin,
              },
            },
          });
        }
        if (!matchesToken(request.headers.authorization, expectedToken)) {
          request.resume();
          return json(
            response,
            401,
            failure("web_unauthorized", "Reconnect to the local Web service"),
          );
        }
        if (url.pathname !== "/api/call" || request.method !== "POST") {
          request.resume();
          return json(
            response,
            405,
            failure("invalid_request", "Use POST /api/call"),
          );
        }
        if (
          request.headers["content-type"]?.split(";")[0] !== "application/json"
        ) {
          request.resume();
          return json(
            response,
            415,
            failure("invalid_request", "JSON input is required"),
          );
        }
        if (Number(request.headers["content-length"] ?? 0) > limit) {
          request.resume();
          return json(
            response,
            413,
            failure("input_too_large", "Input exceeds 1 MiB"),
          );
        }
        let input;
        try {
          input = await body(request);
        } catch (error) {
          return json(
            response,
            error.message === "input_too_large" ? 413 : 400,
            failure("invalid_request", "Invalid or oversized JSON input"),
          );
        }
        if (
          !input ||
          typeof input.method !== "string" ||
          !/^[a-z]+\.[a-z_]+$/.test(input.method) ||
          !input.params ||
          typeof input.params !== "object" ||
          Array.isArray(input.params) ||
          Object.keys(input).some((key) => !["method", "params"].includes(key))
        )
          return json(
            response,
            400,
            failure("invalid_request", "Expected {method, params}"),
          );
        return json(response, 200, await core.call(input.method, input.params));
      }
      if (!["GET", "HEAD"].includes(request.method)) {
        request.resume();
        return json(
          response,
          405,
          failure("invalid_request", "Use GET for the interface"),
        );
      }
      let filename;
      try {
        const relative = decodeURIComponent(
          url.pathname === "/" ? "/index.html" : url.pathname,
        );
        filename = await realpath(path.join(assets, relative));
        if (!filename.startsWith(assets + path.sep))
          throw new Error("outside assets");
      } catch {
        return json(
          response,
          404,
          failure("not_found", "Interface file not found"),
        );
      }
      const type = types[path.extname(filename)];
      if (!type || !(await stat(filename)).isFile())
        return json(
          response,
          404,
          failure("not_found", "Interface file not found"),
        );
      const data = await readFile(filename);
      response.writeHead(200, { "Content-Type": type });
      response.end(request.method === "HEAD" ? undefined : data);
    } catch {
      if (!response.headersSent)
        json(
          response,
          503,
          failure(
            "web_unavailable",
            "Local core is unavailable. Restart the Web service, then refresh before retrying.",
          ),
        );
      else response.destroy();
    }
  });
  server.requestTimeout = 15_000;
  server.headersTimeout = 10_000;
  server.timeout = 130_000;
  server.maxConnections = 32;
  try {
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(options.port ?? 1421, "127.0.0.1", resolve);
    });
  } catch (error) {
    await core.close();
    throw error;
  }
  authority = `127.0.0.1:${server.address().port}`;
  return {
    url: `http://${authority}`,
    async close() {
      server.closeAllConnections();
      await Promise.all([
        new Promise((resolve) => server.close(resolve)),
        core.close(),
      ]);
    },
  };
}

async function main() {
  const options = {};
  const args = process.argv.slice(2);
  if (args.includes("--help")) {
    console.log(
      "Continuo local Web\nnode web/server.mjs [--data-dir PATH] [--binary PATH] [--assets PATH] [--port 1421]\n  [--allow-writes] [--allow-admin] [--allow-sync]\nDefault: read-only, loopback only. Build the CLI and renderer first.",
    );
    return;
  }
  while (args.length) {
    const flag = args.shift();
    const bools = {
      "--allow-writes": "writes",
      "--allow-sync": "sync",
      "--allow-admin": "admin",
    };
    const values = {
      "--data-dir": "dataDir",
      "--binary": "binary",
      "--assets": "assets",
      "--port": "port",
    };
    if (bools[flag]) options[bools[flag]] = true;
    else if (values[flag] && args[0] && !args[0].startsWith("--"))
      options[values[flag]] = args.shift();
    else throw new Error("Unknown or incomplete option; use --help");
  }
  if (options.port !== undefined) {
    options.port = Number(options.port);
    if (
      !Number.isInteger(options.port) ||
      options.port < 1 ||
      options.port > 65535
    )
      throw new Error("Invalid port");
  }
  const instance = await startWebServer(options);
  console.log(
    `Continuo Web: ${instance.url}\nLocal only. Writes: ${!!options.writes}; admin: ${!!options.admin}; sync: ${!!options.sync}.\nKeep this process running. Ctrl+C to stop.`,
  );
  let closing = false;
  const stop = async () => {
    if (closing) return;
    closing = true;
    await instance.close();
    process.exit(0);
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
}
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
)
  main().catch(() => {
    console.error(
      "Web startup failed. Build the CLI/UI, check the chosen data directory, options and port. Use --help.",
    );
    process.exitCode = 1;
  });

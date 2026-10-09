import { invoke, isTauri } from "@tauri-apps/api/core";

export type Kind =
  "identity" | "task" | "capability" | "mcp" | "session" | "device";
export interface Event {
  revision: string;
  entity_id: string;
  name: string;
  kind: Kind;
  data: Record<string, unknown>;
  deleted: boolean;
  timestamp_ms: number;
  device_id: string;
  parents: string[];
}
export interface Entity {
  id: string;
  kind: Kind;
  conflicted: boolean;
  heads: Event[];
}
export interface Status {
  device_id: string;
  counts: Record<Kind, number>;
  conflicts: number;
  event_count: number;
  sync_configured: boolean;
  last_sync: string | null;
}
export interface Adapter {
  id: string;
  name: string;
  executable: string;
  capabilities: Record<string, unknown>;
}
interface Envelope<T> {
  api_version: string;
  ok: boolean;
  data: T;
  error?: { code: string; message: string; details?: unknown };
}
export const desktopAvailable = isTauri();
export interface Tool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
}
export interface Connection {
  mode: "app" | "web";
  permissions: { writes: boolean; admin: boolean; sync: boolean };
  tools?: Tool[];
}
let webToken: string | null = null;
export async function connect(): Promise<Connection> {
  if (desktopAvailable) {
    const catalog = await call<{
      operations: {
        tool: string;
        description: string;
        input_schema: Record<string, unknown>;
      }[];
    }>("system.describe");
    return {
      mode: "app",
      permissions: { writes: true, admin: true, sync: true },
      tools: catalog.operations.map((op) => ({
        name: op.tool,
        description: op.description,
        inputSchema: op.input_schema,
      })),
    };
  }
  try {
    const response = await fetch("/api/bootstrap", {
      headers: { "X-Continuo-Client": "web-v1" },
      cache: "no-store",
      signal: AbortSignal.timeout(5000),
    });
    const envelope = await response.json();
    if (
      !response.ok ||
      !envelope.ok ||
      typeof envelope.data?.token !== "string"
    )
      throw new Error("unavailable");
    webToken = envelope.data.token;
    return envelope.data;
  } catch {
    webToken = null;
    throw new Error(
      "尚未连接本机服务。请启动 Continuo Web 后，从终端显示的本地地址打开。启动步骤见页面提示。",
    );
  }
}
export async function call<T>(
  method: string,
  params: Record<string, unknown> = {},
): Promise<T> {
  let result: Envelope<T>;
  if (desktopAvailable)
    result = await invoke<Envelope<T>>("api_call", { method, params });
  else {
    if (!webToken) await connect();
    try {
      const response = await fetch("/api/call", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Continuo-Client": "web-v1",
          Authorization: `Bearer ${webToken}`,
        },
        body: JSON.stringify({ method, params }),
        signal: AbortSignal.timeout(125_000),
      });
      result = await response.json();
      if (result.api_version !== "1" || typeof result.ok !== "boolean")
        throw new Error("invalid response");
      if (response.status === 401) webToken = null;
    } catch {
      webToken = null;
      throw new Error(
        "本机服务连接中断。请确认终端服务仍在运行，刷新状态后再尝试，避免重复提交修改。",
      );
    }
  }
  if (!result.ok) {
    const messages: Record<string, string> = {
      revision_conflict: "这条记录已在其他入口更新，请刷新后重新查看版本。",
      permission_denied: "当前入口未获授权执行这个操作。",
      invalid_path: "请输入这台设备上的绝对路径。",
      selection_conflict: "当前身份已被其他入口切换，请刷新后再选择。",
      invalid_identity_profile: "请检查身份字段类型和长度，关联记录不能重复。",
      identity_bindings_unavailable:
        "关联的能力或 MCP 缺失、已删除或存在冲突。请检查关联并修复。",
      invalid_identity: "请选择没有冲突且仍有效的身份。",
      invalid_task: "请选择没有冲突且仍有效的任务。",
      sync_not_configured: "请先配置同步仓库和加密密钥。",
      sync_busy: "另一个同步正在进行，请稍后重试。",
      incompatible_workspace:
        "仓库格式或加密密钥不匹配，请核对两台设备的配置。",
      decryption_failed: "无法解密同步内容，请核对密钥和远端数据。",
      sync_push_failed: "远端已变化或上传失败。本地修改已保留，请重新同步。",
      git_failed: "无法访问 Git 仓库，请检查仓库地址与本机 Git 登录。",
      credential_not_allowed: "这里应保存凭据引用，登录凭据需单独保管。",
      invalid_key: "请选择有效的 32 字节加密密钥文件。",
      unsafe_key_permissions: "密钥文件权限过于开放，请设置为仅本人可读写。",
      web_unauthorized: "本机服务已重启，请刷新状态重新连接。",
      web_unavailable: "本机核心已停止，请重启 Web 服务，再刷新确认操作结果。",
      not_found: "这条记录已不存在，请刷新列表。",
    };
    throw new Error(
      messages[result.error?.code ?? ""] ??
        result.error?.message ??
        "操作未完成，请重试。",
    );
  }
  return result.data;
}

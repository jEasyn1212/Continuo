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
  permissions: {
    writes: boolean;
    admin: boolean;
    sync: boolean;
    probes?: boolean;
  };
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
      permissions: { writes: true, admin: true, sync: true, probes: true },
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
      invalid_mcp_profile:
        "请检查 MCP 定义。注册名称不含空格，本机路径留在映射中，敏感值使用引用。",
      invalid_mcp_mapping:
        "请检查本机绝对路径、参数数组与 NAME=env:VARIABLE 引用。",
      mcp_mapping_conflict:
        "定义或本机映射已经变化，草稿已保留，请重新核对版本。",
      mcp_unavailable: "MCP 定义已删除或存在并发版本，请先修复。",
      mcp_not_ready: "MCP 定义或本机环境尚未就绪，请先处理检查项。",
      mcp_adapter_unsupported:
        "适配器尚未支持这些字段。cwd 暂不能安全写入注册格式，请留空或使用自己审查过的启动器。",
      mcp_confirmation_required: "运行服务命令前，需要明确确认这一版命令。",
      mcp_probe_platform_unsupported:
        "当前只支持 macOS/Linux 的有界连接检查，其他平台可先保存定义和注册计划。",
      mcp_busy: "这个 MCP 的连接检查已在运行，可等待或取消。",
      mcp_probe_conflict: "这次连接检查已经结束或被替换，请刷新状态。",
      invalid_capability_profile:
        "请检查能力正文、版本、来源与依赖字段。来源使用不含凭据或查询参数的 HTTPS 地址或 project:相对路径。",
      capability_unavailable: "能力已删除、类型错误或存在并发版本，请先修复。",
      capability_not_ready:
        "能力或依赖尚待检查、缺失、存在冲突或不适用于当前 agent，请在能力模块处理后重试。",
      capability_dependency_cycle: "能力依赖形成循环，请取消循环关联。",
      capability_dependency_unavailable:
        "依赖能力缺失、已删除或有冲突，请清除关联或先修复依赖。",
      capability_graph_too_large: "能力依赖图或合并正文过大，请减少本次选择。",
      capability_unsupported: "当前适配器尚未支持能力应用。",
      invalid_session_profile:
        "会话字段不符合要求。路径、账号和原始历史不要存入同步记录；引用使用 project:相对路径或无凭据的 HTTPS。",
      session_unavailable: "会话已删除或有并发版本，请先修复。",
      session_relation_unavailable:
        "关联任务或身份缺失、已删除或有冲突，请更换或清除关联。",
      session_identity_mismatch: "会话与任务身份不同，请明确调整关联。",
      session_transition_requires_reason:
        "状态变化需要新增原因，请使用会话状态记录入口。",
      invalid_session_transition: "请选择当前允许的会话状态变化。",
      invalid_session_mapping: "请填写本机绝对路径和账号引用，不填写秘密。",
      session_mapping_conflict:
        "会话或本机环境已更新，草稿已保留，请刷新核对版本。",
      session_not_ready:
        "请处理会话检查项：原生恢复需本机确认，跨 agent 接续需可用任务和身份。",
      session_adapter_unsupported: "这个 adapter 尚未实现原生会话恢复计划。",
      invalid_task_profile:
        "任务字段、状态或产物引用不符合要求。受阻需填写阻碍，完成需填写结论；产物使用 project:相对路径或不含凭据、查询参数的 HTTPS 地址。",
      invalid_task_identity:
        "关联身份缺失、已删除或存在冲突，请更换或清除任务的身份关联。",
      task_transition_requires_reason:
        "状态变化必须记录新的原因，请使用任务状态流转接口。",
      invalid_task_transition:
        "不能直接进入这个状态，请使用当前允许的状态流转。",
      task_not_ready:
        "接续前请填写目标、下一步，并确认任务仍开放且关联身份有效。",
      task_identity_mismatch:
        "所选身份与任务关联的身份不同，请明确修改任务的身份关联。",
      invalid_task: "请选择没有冲突且仍有效的任务。",
      sync_config_changed: "同步设置已变化，请刷新并核对，草稿已保留。",
      sync_disabled: "这台设备已停用同步，请核对设置后再恢复。",
      sync_key_missing: "密钥文件不存在或无法读取，请恢复原密钥后重试。",
      sync_key_changed: "配置对应的密钥文件内容已改变，请找回原密钥。",
      sync_remote_history_missing:
        "曾确认的远端历史已缺失，停止发布；检查分支与备份，不自动覆盖。",
      sync_timeout: "同步达到超时。本地数据保留，可重新读取后重试。",
      sync_cancelled: "已取消操作，发布可能已完成，重试会读取并去重。",
      sync_job_exists: "操作 ID 已使用，先查看精确结果，再用新的 ID 重试。",
      sync_job_changed: "本次同步已结束或被替换，请刷新状态。",
      sync_job_missing: "没有找到这个同步操作。",
      git_unavailable: "本机无法启动 Git，请检查安装和 PATH。",
      sync_transport_unsupported: "有界 Git 传输当前支持 macOS/Linux。",
      sync_limit: "远端规模或响应超过上限，数据未部分导入。",
      invalid_sync_repository: "远端分支不符合协议，请核对仓库。",
      missing_parent: "事件历史不完整，不能部分导入。",
      history_version_missing: "所选版本不属于这条记录，请重新核对历史。",
      invalid_restore_source: "请选择非删除的历史版本。",
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

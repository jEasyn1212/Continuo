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
export async function call<T>(
  method: string,
  params: Record<string, unknown> = {},
): Promise<T> {
  if (!desktopAvailable)
    throw new Error("请通过桌面应用打开，浏览器预览不连接本地数据。");
  const result = await invoke<Envelope<T>>("api_call", { method, params });
  if (!result.ok) {
    const messages: Record<string, string> = {
      revision_conflict: "这条记录已在其他入口更新，请刷新后重新查看版本。",
      permission_denied: "当前入口未获授权执行这个操作。",
      invalid_path: "请输入这台设备上的绝对路径。",
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

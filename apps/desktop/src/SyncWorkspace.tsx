import { useEffect, useRef, useState } from "react";
import { call, Entity, Event, Kind } from "./api";
import { useUnsavedChanges } from "./useUnsavedChanges";
interface Job {
  run_id: string;
  mode: string;
  state: string;
  phase: string;
  publication: string;
  cancel_requested: boolean;
  error?: { code: string };
  result?: unknown;
}
interface Inspection {
  configured: boolean;
  config: { remote: string; key_file: string } | null;
  config_revision: string;
  enabled: boolean;
  key_status: { ready: boolean; code?: string };
  job: Job | null;
  local_events: number;
  pending_upload: number | null;
  pending_download: number | null;
  deleted_records: number;
  conflicts: { id: string; kind: Kind; name: string }[];
  last_success: unknown;
}
interface History {
  versions: { revision: string; name: string; deleted: boolean }[];
  total_versions: number;
  truncated: boolean;
}
const phases: Record<string, string> = {
  checking: "检查本机配置",
  contacting: "连接用户存储",
  fetching: "读取远端",
  validating: "校验版本与加密",
  importing: "导入已验证事件",
  encrypting: "准备加密上传",
  publishing: "发布版本",
};
const states: Record<string, string> = {
  running: "正在同步",
  succeeded: "已完成",
  failed: "失败，数据保留",
  cancelled: "已取消，可安全重试",
  timeout: "超时，可安全重试",
  interrupted: "上次操作被中断",
};
const hints: Record<string, string> = {
  sync_key_missing: "密钥文件缺失。找回原密钥或备份，新密钥不能解开旧仓库。",
  sync_key_changed: "密钥文件内容已改变，请找回原密钥。",
  invalid_key: "密钥需为已有的 32 字节文件。",
  unsafe_key_permissions: "密钥权限过于开放，需仅允许本人读取。",
  incompatible_workspace: "远端格式或密钥不一致，核对仓库与原密钥。",
  sync_remote_history_missing:
    "曾确认的远端历史缺失，已停止发布。先检查分支和备份，不自动覆盖或接受回退。",
  missing_parent: "远端因果历史不完整，不能部分导入。",
  decryption_failed: "密钥错误或密文被篡改，未部分导入。",
  git_failed: "检查仓库访问、本机 Git 登录和网络，再重试。",
  git_unavailable: "无法启动 Git，请检查安装和 PATH。",
  sync_push_failed: "远端变化或发布失败。本地数据保留，重新读取并去重重试。",
  sync_disabled: "当前设备停用同步，可核对配置后恢复。",
  sync_timeout: "操作达到超时，可以重新读取远端后重试。",
  sync_cancelled: "本次操作已取消，发布可能已完成，重试会读取并去重。",
  sync_limit: "远端规模或响应超过上限，不部分导入。",
  unsupported_schema: "数据版本不受支持，需使用兼容客户端。",
  invalid_sync_repository: "远端协议文件无效，请核对仓库。",
};
export function SyncWorkspace({
  writable,
  admin,
  networkAllowed,
  refreshSignal,
  onChange,
  onOpenModule,
  onDirtyChange,
}: {
  writable: boolean;
  admin: boolean;
  networkAllowed: boolean;
  refreshSignal: unknown;
  onChange: () => Promise<void>;
  onOpenModule: (kind: Kind) => void;
  onDirtyChange?: (v: boolean) => void;
}) {
  const [state, setState] = useState<Inspection | null>(null),
    [records, setRecords] = useState<Entity[]>([]),
    [config, setConfig] = useState<{
      revision: string;
      remote: string;
      key_file: string;
    } | null>(null),
    [dirty, setDirty] = useState(false),
    [busy, setBusy] = useState(false),
    [running, setRunning] = useState(false),
    [error, setError] = useState(""),
    [conflict, setConflict] = useState<Entity | null>(null),
    [mergeName, setMergeName] = useState(""),
    [mergeData, setMergeData] = useState(""),
    [mergeDeleted, setMergeDeleted] = useState(false),
    [restoreId, setRestoreId] = useState(""),
    [history, setHistory] = useState<History | null>(null),
    [source, setSource] = useState(""),
    [sourceEvent, setSourceEvent] = useState<Event | null>(null);
  const mounted = useRef(true),
    owned = useRef<string | null>(null);
  useUnsavedChanges(dirty, onDirtyChange);
  async function load() {
    const [s, r] = await Promise.all([
      call<Inspection>("sync.inspect"),
      call<{ entities: Entity[] }>("entity.list", { include_deleted: true }),
    ]);
    if (mounted.current) {
      setState(s);
      setRecords(r.entities);
    }
  }
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (owned.current)
        void call("sync.cancel", { run_id: owned.current }).catch(() => {});
    };
  }, []);
  useEffect(() => {
    void load().catch((e) => setError(String(e)));
  }, [refreshSignal]);
  useEffect(() => {
    if (!running && state?.job?.state !== "running") return;
    const timer = setInterval(() => void load().catch(() => {}), 250);
    return () => clearInterval(timer);
  }, [running, state?.job?.state]);
  async function action(fn: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await fn();
      await load();
      await onChange();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }
  function discard() {
    if (dirty && !window.confirm("放弃尚未保存的同步配置或冲突整理？"))
      return false;
    setDirty(false);
    setConfig(null);
    setConflict(null);
    return true;
  }
  function editConfig() {
    if (!state || !discard()) return;
    setConfig({
      revision: state.config_revision,
      remote: state.config?.remote ?? "",
      key_file: state.config?.key_file ?? "",
    });
  }
  async function run(preview: boolean) {
    if (!state) return;
    if (
      !preview &&
      !window.confirm(
        "同步到你配置的存储：\n" +
          state.config?.remote +
          "\n\n将上传本工作区的可携带版本记录；本机路径、设备映射、登录秘密和密钥不上传。取消后发布仍可能已经完成，重试会重新读取并去重。",
      )
    )
      return;
    const id = crypto.randomUUID();
    owned.current = id;
    setRunning(true);
    setError("");
    try {
      await call(preview ? "sync.preview" : "sync.run", {
        run_id: id,
        timeout_ms: 30000,
      });
      await load();
      await onChange();
    } catch (e) {
      if (mounted.current) {
        setError(e instanceof Error ? e.message : String(e));
        await load().catch(() => {});
      }
    } finally {
      owned.current = null;
      if (mounted.current) setRunning(false);
    }
  }
  async function openConflict(id: string) {
    if (!discard()) return;
    const e = await call<Entity>("entity.get", { id });
    setConflict(e);
    setMergeName(e.heads[0].name);
    setMergeData(JSON.stringify(e.heads[0].data, null, 2));
    setMergeDeleted(e.heads[0].deleted);
  }
  async function openHistory(id: string) {
    setRestoreId(id);
    setSource("");
    setSourceEvent(null);
    setHistory(id ? await call<History>("entity.history", { id }) : null);
  }
  const active = running || state?.job?.state === "running",
    deleted = records.filter((r) => r.heads.length === 1 && r.heads[0].deleted),
    restoring = deleted.find((r) => r.id === restoreId),
    canNetwork =
      networkAllowed &&
      writable &&
      state?.configured &&
      state.enabled &&
      state.key_status.ready;
  return (
    <div className="sync-workspace">
      <div className="page-heading">
        <div>
          <div className="eyebrow">YOUR STORAGE, YOUR DATA</div>
          <h1>同步、检查与恢复。</h1>
          <p>使用你自己的存储，Continuo 无需运营统一服务端。</p>
        </div>
        <button disabled={busy} onClick={() => void action(load)}>
          刷新同步状态
        </button>
      </div>
      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      <div className="mcp-status-grid">
        <div>
          <small>本机记录</small>
          <strong>
            {state?.local_events ?? "…"} 个版本 · {state?.deleted_records ?? 0}{" "}
            条删除记录
          </strong>
        </div>
        <div>
          <small>待上传 / 待拉取</small>
          <strong>
            {state?.pending_upload ?? "待检查"} /{" "}
            {state?.pending_download ?? "待检查"}
          </strong>
          <p className="help">
            基于最近一次已验证快照，离线时不是实时远端状态。
          </p>
        </div>
        <div>
          <small>同步存储</small>
          <strong>
            {state?.configured
              ? state.enabled
                ? "已配置"
                : "这台设备已停用"
              : "尚未配置"}
          </strong>
          <p className="help">
            {state?.config?.remote ?? "选择专用私有仓库，先核对再同步。"}
          </p>
        </div>
        <div>
          <small>密钥与运行状态</small>
          <strong>
            {state?.key_status.ready ? "本机密钥可读" : "本机密钥待处理"}
          </strong>
          <p className="help">
            {state?.job
              ? (states[state.job.state] ?? state.job.state)
              : "尚未运行"}
          </p>
        </div>
      </div>
      {state?.key_status.code && state.configured && (
        <div className="notice">
          {hints[state.key_status.code] ?? state.key_status.code}
        </div>
      )}
      <section className="settings-form">
        <div className="detail-title">
          <h2>存储与本机设置</h2>
          <button
            disabled={!state || !writable || !admin || busy || active}
            onClick={editConfig}
          >
            配置同步存储
          </button>
        </div>
        <p className="help">
          设备使用同一工作区密钥，本机 Git
          负责认证。配置、密钥和设备映射分别留在本机。
        </p>
        {config && (
          <>
            <label>
              用户存储地址
              <input
                value={config.remote}
                placeholder="https://github.com/you/continuo-data.git"
                onChange={(e) => {
                  setConfig({ ...config, remote: e.target.value });
                  setDirty(true);
                }}
              />
            </label>
            <label>
              本机工作区密钥文件
              <input
                value={config.key_file}
                placeholder="已有密钥文件的绝对路径"
                onChange={(e) => {
                  setConfig({ ...config, key_file: e.target.value });
                  setDirty(true);
                }}
              />
            </label>
            <p className="help">
              加入已有工作区需原密钥。新工作区才生成新密钥；丢失密钥无法从
              GitHub 还原明文。
            </p>
            <div className="form-actions">
              <button
                disabled={
                  !writable ||
                  !admin ||
                  busy ||
                  !config.remote ||
                  !config.key_file
                }
                onClick={() =>
                  void action(async () => {
                    await call("sync.configure", {
                      remote: config.remote,
                      key_file: config.key_file,
                      expected_config_revision: config.revision,
                    });
                    setConfig(null);
                    setDirty(false);
                  })
                }
              >
                保存同步配置
              </button>
              <button
                disabled={!writable || !admin || busy || !config.key_file}
                onClick={() => {
                  if (
                    window.confirm(
                      "仅为新工作区生成密钥：\n" +
                        config.key_file +
                        "\n\n不覆盖已有文件。新密钥不能解开原工作区历史。",
                    )
                  )
                    void action(async () => {
                      await call("sync.key_generate", {
                        path: config.key_file,
                      });
                    });
                }}
              >
                为新工作区生成密钥
              </button>
              <button onClick={() => discard()}>取消配置编辑</button>
            </div>
          </>
        )}
        <div className="form-actions">
          <button
            disabled={!canNetwork || busy || active || dirty}
            onClick={() => void run(true)}
          >
            检查远端待同步
          </button>
          <button
            className="primary"
            disabled={!canNetwork || busy || active || dirty}
            onClick={() => void run(false)}
          >
            同步 / 安全重试
          </button>
          <button
            disabled={
              !writable || !admin || busy || active || !state?.configured
            }
            onClick={() =>
              void action(async () => {
                await call("sync.set_enabled", {
                  expected_config_revision: state?.config_revision,
                  enabled: !state?.enabled,
                });
              })
            }
          >
            {state?.enabled ? "停用本机同步" : "恢复本机同步"}
          </button>
        </div>
        {!networkAllowed && (
          <p className="help">
            当前入口没有网络同步权限。Web/MCP 需用户明确以 --allow-writes
            --allow-sync 启动；管理配置另需 --allow-admin。本页面不扩展权限。
          </p>
        )}
        <p className="help">
          本机停用保留配置与数据，不撤销其他设备密钥，也不擦除远端历史。
        </p>
        {state?.job && (
          <div className="identity-check">
            <h3>{states[state.job.state] ?? state.job.state}</h3>
            <p>
              {phases[state.job.phase] ?? state.job.phase} ·{" "}
              {state.job.mode === "preview" ? "仅预检" : "交换版本"}
            </p>
            <p>本次操作：{state.job.run_id}</p>
            {state.job.error && (
              <p>{hints[state.job.error.code] ?? state.job.error.code}</p>
            )}
            {state.job.publication === "unknown" && (
              <p>
                发布结果不确定，可能已完成。重新读取远端，再按版本去重重试。
              </p>
            )}
            {state.job.state === "running" && (
              <button
                disabled={!writable || busy || state.job.cancel_requested}
                onClick={() =>
                  void action(async () => {
                    await call("sync.cancel", { run_id: state.job?.run_id });
                  })
                }
              >
                {state.job.cancel_requested ? "已请求取消" : "取消本次同步"}
              </button>
            )}
            <details>
              <summary>结果与精确状态</summary>
              <pre>{JSON.stringify(state.job, null, 2)}</pre>
            </details>
          </div>
        )}
      </section>
      <section className="settings-form">
        <h2>并发冲突</h2>
        <p className="help">
          保留所有并发版本，整理会引用全部当前版本，不按设备时钟选赢家。
        </p>
        {state?.conflicts.map((c) => (
          <div className="sync-record" key={c.id}>
            <span>
              {c.name} · {c.kind}
            </span>
            <button
              disabled={busy || active}
              onClick={() => void action(() => openConflict(c.id))}
            >
              核对并整理版本
            </button>
            <button onClick={() => onOpenModule(c.kind)}>在所属模块查看</button>
          </div>
        ))}
        {state?.conflicts.length === 0 && (
          <p className="help">没有待整理的并发版本。</p>
        )}
        {conflict && (
          <>
            <h3>{conflict.heads[0].name}</h3>
            {conflict.heads.map((h) => (
              <article className="version" key={h.revision}>
                <p>
                  {h.revision} · {h.deleted ? "删除版本" : "保留版本"} ·{" "}
                  {h.device_id}
                </p>
                <pre>{JSON.stringify(h.data, null, 2)}</pre>
                <button
                  disabled={!writable || busy || active}
                  onClick={() => {
                    setMergeName(h.name);
                    setMergeData(JSON.stringify(h.data, null, 2));
                    setMergeDeleted(h.deleted);
                    setDirty(true);
                  }}
                >
                  以此内容整理
                </button>
              </article>
            ))}
            <label>
              整理后的名称
              <input
                value={mergeName}
                disabled={!writable}
                onChange={(e) => {
                  setMergeName(e.target.value);
                  setDirty(true);
                }}
              />
            </label>
            <label>
              整理后的记录内容
              <textarea
                value={mergeData}
                disabled={!writable}
                onChange={(e) => {
                  setMergeData(e.target.value);
                  setDirty(true);
                }}
              />
            </label>
            <label className="binding-choice">
              <input
                type="checkbox"
                checked={mergeDeleted}
                disabled={!writable}
                onChange={(e) => {
                  setMergeDeleted(e.target.checked);
                  setDirty(true);
                }}
              />
              合并为删除标记
            </label>
            <div className="form-actions">
              <button
                disabled={!writable || busy || active}
                onClick={() =>
                  void action(async () => {
                    await call("entity.resolve", {
                      id: conflict.id,
                      expected_heads: conflict.heads.map((h) => h.revision),
                      name: mergeName,
                      data: JSON.parse(mergeData),
                      deleted: mergeDeleted,
                    });
                    setConflict(null);
                    setDirty(false);
                  })
                }
              >
                保存冲突整理
              </button>
              <button onClick={() => discard()}>取消冲突整理</button>
            </div>
          </>
        )}
      </section>
      <section className="settings-form">
        <h2>恢复删除记录</h2>
        <p className="help">
          恢复所选历史版本，保留删除标记和因果历史。不恢复原生 agent
          历史、丢失密钥或设备映射；失效关联需先修复。
        </p>
        <label>
          已删除的记录
          <select
            disabled={!state || busy}
            value={restoreId}
            onChange={(e) => void action(() => openHistory(e.target.value))}
          >
            <option value="">选择要恢复的记录</option>
            {deleted.map((r) => (
              <option key={r.id} value={r.id}>
                {r.heads[0].name} · {r.kind}
              </option>
            ))}
          </select>
        </label>
        {history && (
          <>
            <p className="help">
              共 {history.total_versions} 个版本，显示最近 200 个因果版本。
              {history.truncated ? "更早版本可填写已知 revision。" : ""}
            </p>
            <label>
              要恢复的历史 revision
              <input
                list="restore-versions"
                value={source}
                onChange={(e) => {
                  setSource(e.target.value);
                  setSourceEvent(null);
                }}
              />
            </label>
            <datalist id="restore-versions">
              {history.versions
                .filter((v) => !v.deleted)
                .map((v) => (
                  <option key={v.revision} value={v.revision}>
                    {v.name}
                  </option>
                ))}
            </datalist>
            <div className="form-actions">
              <button
                disabled={!source || busy}
                onClick={() =>
                  void action(async () =>
                    setSourceEvent(
                      await call<Event>("entity.history_version", {
                        id: restoreId,
                        revision: source,
                      }),
                    ),
                  )
                }
              >
                查看历史版本
              </button>
              <button
                disabled={
                  !writable ||
                  busy ||
                  active ||
                  !restoring ||
                  !sourceEvent ||
                  sourceEvent.deleted
                }
                onClick={() =>
                  void action(async () => {
                    await call("entity.restore", {
                      id: restoreId,
                      expected_revision: restoring?.heads[0].revision,
                      source_revision: sourceEvent?.revision,
                    });
                    setRestoreId("");
                    setSource("");
                    setSourceEvent(null);
                    setHistory(null);
                  })
                }
              >
                恢复所选版本
              </button>
              <button
                onClick={() => {
                  setRestoreId("");
                  setHistory(null);
                  setSourceEvent(null);
                }}
              >
                取消恢复
              </button>
            </div>
            {sourceEvent && (
              <pre aria-label="恢复版本预览">
                {JSON.stringify(sourceEvent, null, 2)}
              </pre>
            )}
          </>
        )}
      </section>
      <details>
        <summary>同步与恢复边界</summary>
        <p className="help">
          已提供事件同步、冲突与逻辑删除恢复。设备的密码学撤销、密钥轮换/找回、自动同步和原生状态迁移尚未实现。历史缺失检查仅针对本机、同一地址/密钥下曾确认的事件。Git
          有超时和本机进程清理，强制关闭或逃逸后台程序不保证清理，远端发布可能已完成。
        </p>
        <pre>{JSON.stringify(state?.last_success, null, 2)}</pre>
      </details>
    </div>
  );
}

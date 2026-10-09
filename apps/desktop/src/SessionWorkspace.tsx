import { useEffect, useState } from "react";
import { Adapter, call, Entity, Event } from "./api";
import { useUnsavedChanges } from "./useUnsavedChanges";
type Issue = { code: string };
interface Mapping {
  revision: string;
  cleared: boolean;
  executable: string;
  cwd: string;
  account_ref: string | null;
  runtime_version: string;
  state_present_confirmed: boolean;
}
interface Inspection {
  session: Event;
  profile: Record<string, unknown>;
  mapping: Mapping | null;
  allowed_transitions: string[];
  relationships: Issue[];
  resume: { ready: boolean; issues: Issue[] };
  continuation: { ready: boolean; issues: Issue[] };
  task_context: { task?: Event } | null;
  identity_context: unknown;
}
interface Draft {
  id?: string;
  revision?: string;
  heads?: string[];
  base: Record<string, unknown>;
  name: string;
  agent: string;
  native_session_id: string;
  task_id: string;
  identity_id: string;
  origin_device_id: string;
  environment_ref: string;
  source: string;
  source_ref: string;
  summary: string;
}
function draftOf(e?: Event, r?: Entity): Draft {
  const d = e?.data ?? {};
  return {
    id: r?.id,
    revision: e?.revision,
    heads: r?.conflicted ? r.heads.map((h) => h.revision) : undefined,
    base: d,
    name: e?.name ?? "",
    agent: String(d.agent ?? "claude-code"),
    native_session_id: String(d.native_session_id ?? ""),
    task_id: String(d.task_id ?? ""),
    identity_id: String(d.identity_id ?? ""),
    origin_device_id: String(d.origin_device_id ?? ""),
    environment_ref: String(d.environment_ref ?? ""),
    source: String(d.source ?? "manual"),
    source_ref: String(d.source_ref ?? ""),
    summary: String(d.summary ?? ""),
  };
}
const states: Record<string, string> = {
  registered: "已登记",
  active: "活动（记录）",
  paused: "已暂停",
  closed: "已结束",
  cancelled: "已取消",
};
const labels: Record<string, string> = {
  session_agent_missing: "尚未记录来源 agent",
  session_closed: "先重新打开已结束或取消的记录",
  session_task_missing: "关联任务后才能生成接续材料",
  session_task_not_ready:
    "在任务模块完善目标、下一步与身份，重新打开已结束任务",
  session_task_unavailable: "关联任务缺失、已删除或有冲突",
  session_identity_unavailable: "关联身份或其能力/MCP 需要修复",
  session_identity_mismatch: "会话与任务指定身份不一致",
  native_session_missing: "尚未填写原生会话 ID",
  session_adapter_unsupported: "没有这个 agent 的原生恢复适配器",
  session_mapping_missing: "这台设备尚未配置恢复环境",
  session_mapping_stale:
    "会话 ID、来源设备或环境引用已变化，请重新核对本机环境",
  session_executable_missing: "本机找不到可执行文件",
  session_executable_not_executable: "文件没有执行权限",
  session_cwd_missing: "工作目录不存在",
  native_state_unconfirmed: "尚未确认本机账号下存在该原生会话",
};
export function SessionWorkspace({
  writable,
  admin,
  adapters,
  refreshSignal,
  onChange,
  onDirtyChange,
}: {
  writable: boolean;
  admin: boolean;
  adapters: Adapter[];
  refreshSignal: unknown;
  onChange: () => Promise<void>;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [records, setRecords] = useState<Entity[]>([]),
    [selected, setSelected] = useState<string | null>(null),
    [draft, setDraft] = useState<Draft | null>(null),
    [inspection, setInspection] = useState<Inspection | null>(null),
    [mapDraft, setMapDraft] = useState<
      (Mapping & { session_revision: string }) | null
    >(null),
    [dirty, setDirty] = useState(false),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [query, setQuery] = useState(""),
    [filter, setFilter] = useState("all"),
    [transition, setTransition] = useState(""),
    [reason, setReason] = useState(""),
    [target, setTarget] = useState("codex"),
    [cwd, setCwd] = useState(""),
    [packet, setPacket] = useState<{ prompt: string } | null>(null),
    [plan, setPlan] = useState<unknown>(null),
    [deleting, setDeleting] = useState(false);
  useUnsavedChanges(dirty || Boolean(reason), onDirtyChange);
  const sessions = records.filter(
      (r) => r.kind === "session" && !r.heads.every((h) => h.deleted),
    ),
    current = sessions.find((r) => r.id === selected),
    revisionKey = current?.heads.map((h) => h.revision).join(","),
    ready = Boolean(
      current &&
      !current.conflicted &&
      inspection?.session.revision === current.heads[0].revision,
    );
  const contextKey = records
    .map((r) => r.heads.map((h) => h.revision).join(","))
    .join(";");
  const linked = (kind: string) =>
    records.filter(
      (r) => r.kind === kind && !r.conflicted && !r.heads[0].deleted,
    );
  async function load() {
    setRecords(
      (
        await call<{ entities: Entity[] }>("entity.list", {
          include_deleted: true,
        })
      ).entities,
    );
  }
  useEffect(() => {
    let cancelled = false;
    void call<{ entities: Entity[] }>("entity.list", { include_deleted: true })
      .then((r) => {
        if (!cancelled) setRecords(r.entities);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [refreshSignal]);
  useEffect(() => {
    let cancelled = false;
    setPacket(null);
    setPlan(null);
    setTransition("");
    if (current && !current.conflicted)
      void call<Inspection>("session.inspect", { id: current.id })
        .then((r) => {
          if (!cancelled) setInspection(r);
        })
        .catch((e) => {
          if (!cancelled) {
            setInspection(null);
            setError(String(e));
          }
        });
    else setInspection(null);
    return () => {
      cancelled = true;
    };
  }, [selected, revisionKey, contextKey]);
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
      setBusy(false);
    }
  }
  function choose(id: string | null, d: Draft | null = null) {
    if ((dirty || reason) && !window.confirm("放弃尚未保存的会话修改？"))
      return;
    setSelected(id);
    setDraft(d);
    setMapDraft(null);
    setDirty(false);
    setReason("");
    setError("");
    setPacket(null);
    setPlan(null);
    setDeleting(false);
    if (id !== selected) setInspection(null);
  }
  function change(key: keyof Draft, value: string) {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
    setDirty(true);
  }
  async function save() {
    if (!draft) return;
    const data = {
      ...draft.base,
      agent: draft.agent,
      native_session_id: draft.native_session_id || null,
      task_id: draft.task_id || null,
      identity_id: draft.identity_id || null,
      origin_device_id: draft.origin_device_id || null,
      environment_ref: draft.environment_ref || null,
      source: draft.source,
      source_ref: draft.source_ref || null,
      summary: draft.summary,
    };
    const r = await call<Entity>(
      draft.heads
        ? "entity.resolve"
        : draft.id
          ? "entity.update"
          : "session.register",
      draft.heads
        ? { id: draft.id, expected_heads: draft.heads, name: draft.name, data }
        : draft.id
          ? {
              id: draft.id,
              expected_revision: draft.revision,
              name: draft.name,
              data,
            }
          : { name: draft.name, data },
    );
    setSelected(r.id);
    setDraft(null);
    setDirty(false);
  }
  function editMap() {
    if (!inspection) return;
    const m = inspection.mapping;
    setMapDraft({
      revision: m?.revision ?? "none",
      cleared: false,
      session_revision: inspection.session.revision,
      executable: m?.executable ?? "",
      cwd: m?.cwd ?? "",
      account_ref: m?.account_ref ?? "",
      runtime_version: m?.runtime_version ?? "",
      state_present_confirmed: false,
    });
    setDraft(null);
    setDirty(false);
  }
  function updateMap(key: keyof Mapping, value: string | boolean) {
    setMapDraft((d) => (d ? { ...d, [key]: value } : d));
    setDirty(true);
  }
  async function saveMap() {
    if (!mapDraft || !selected) return;
    await call("session.map", {
      id: selected,
      expected_revision: mapDraft.session_revision,
      expected_mapping_revision: mapDraft.revision,
      mapping: {
        executable: mapDraft.executable,
        cwd: mapDraft.cwd,
        account_ref: mapDraft.account_ref || null,
        runtime_version: mapDraft.runtime_version,
        state_present_confirmed: mapDraft.state_present_confirmed,
      },
    });
    setMapDraft(null);
    setDirty(false);
    setPlan(null);
    setInspection(await call<Inspection>("session.inspect", { id: selected }));
  }
  async function contextPlan(native = false, launch = false) {
    if (!inspection || !selected) return;
    const common = {
      id: selected,
      expected_revision: inspection.session.revision,
    };
    if (native) {
      setPlan(
        await call("session.resume_plan", {
          ...common,
          expected_mapping_revision: inspection.mapping?.revision,
        }),
      );
      setPacket(null);
    } else {
      const p = {
        ...common,
        target_agent: target,
        expected_task_revision: inspection.task_context?.task?.revision,
      };
      if (launch) setPlan(await call("session.continue_plan", { ...p, cwd }));
      else {
        setPacket(await call("session.packet", p));
        setPlan(null);
      }
    }
  }
  const issues = inspection
    ? [
        ...new Map(
          [
            ...inspection.relationships,
            ...inspection.resume.issues,
            ...inspection.continuation.issues,
          ].map((i) => [i.code, i]),
        ).values(),
      ]
    : [];
  return (
    <section className="entity-workspace session-workspace">
      <aside className="entity-list">
        <div className="list-toolbar">
          <input
            aria-label="搜索会话"
            placeholder="搜索会话、摘要或原生 ID"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <select
            aria-label="筛选会话"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          >
            <option value="all">全部会话</option>
            <option value="conflicts">并发冲突</option>
            {Object.entries(states).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
          <button
            disabled={!writable || busy}
            onClick={() => choose(null, draftOf())}
          >
            ＋ 登记会话
          </button>
        </div>
        {sessions
          .filter(
            (r) =>
              (filter === "all" ||
                (filter === "conflicts" && r.conflicted) ||
                (!r.conflicted &&
                  (r.heads[0].data.status ?? "registered") === filter)) &&
              r.heads.some((h) =>
                `${h.name} ${h.data.summary ?? ""} ${h.data.native_session_id ?? ""}`
                  .toLowerCase()
                  .includes(query.toLowerCase()),
              ),
          )
          .map((r) => (
            <button
              className={`entity ${r.id === selected ? "selected" : ""}`}
              key={r.id}
              disabled={busy}
              onClick={() => choose(r.id)}
            >
              <strong>{r.heads[0].name}</strong>
              <span>
                {r.conflicted
                  ? "需要合并"
                  : states[String(r.heads[0].data.status ?? "registered")]}{" "}
                · {String(r.heads[0].data.agent ?? "未指定")}
              </span>
            </button>
          ))}
      </aside>
      <div className="detail">
        {error && (
          <div className="error" role="alert">
            {error}
          </div>
        )}
        {draft ? (
          <>
            <h2>
              {draft.heads
                ? "整理会话冲突"
                : draft.id
                  ? "编辑会话"
                  : "登记原生会话引用"}
            </h2>
            <p className="muted">
              登记你提供的标识和摘要，不读取账号历史。原生 ID
              是引用，不证明另一台设备可以恢复。
            </p>
            <div className="form-grid">
              <label>
                会话名称
                <input
                  value={draft.name}
                  onChange={(e) => change("name", e.target.value)}
                />
              </label>
              <label>
                来源 agent
                <select
                  value={draft.agent}
                  onChange={(e) => change("agent", e.target.value)}
                >
                  {!adapters.some((a) => a.id === draft.agent) && (
                    <option value={draft.agent}>
                      {draft.agent || "未指定"}
                    </option>
                  )}
                  {adapters.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                原生会话 ID
                <input
                  value={draft.native_session_id}
                  onChange={(e) => change("native_session_id", e.target.value)}
                />
              </label>
              <label>
                来源设备 ID
                <input
                  value={draft.origin_device_id}
                  placeholder="新记录留空登记为当前设备"
                  onChange={(e) => change("origin_device_id", e.target.value)}
                />
              </label>
              <label>
                关联任务
                <select
                  value={draft.task_id}
                  onChange={(e) => {
                    const t = linked("task").find(
                      (t) => t.id === e.target.value,
                    );
                    setDraft((d) =>
                      d
                        ? {
                            ...d,
                            task_id: e.target.value,
                            identity_id: String(
                              t?.heads[0].data.identity_id ?? d.identity_id,
                            ),
                          }
                        : d,
                    );
                    setDirty(true);
                  }}
                >
                  <option value="">暂不关联任务</option>
                  {draft.task_id &&
                    !linked("task").some((t) => t.id === draft.task_id) && (
                      <option value={draft.task_id}>关联任务待修复</option>
                    )}
                  {linked("task").map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.heads[0].name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                关联身份
                <select
                  value={draft.identity_id}
                  onChange={(e) => change("identity_id", e.target.value)}
                >
                  <option value="">采用任务身份或无身份</option>
                  {draft.identity_id &&
                    !linked("identity").some(
                      (t) => t.id === draft.identity_id,
                    ) && (
                      <option value={draft.identity_id}>关联身份待修复</option>
                    )}
                  {linked("identity").map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.heads[0].name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                环境引用
                <input
                  value={draft.environment_ref}
                  placeholder="project:relative/path 或 HTTPS"
                  onChange={(e) => change("environment_ref", e.target.value)}
                />
              </label>
              <label>
                来源类型
                <select
                  value={draft.source}
                  onChange={(e) => change("source", e.target.value)}
                >
                  <option value="manual">用户手动登记</option>
                  <option value="referenced">用户提供来源引用</option>
                </select>
              </label>
              <label>
                来源引用
                <input
                  value={draft.source_ref}
                  placeholder="仅记录，不自动读取"
                  onChange={(e) => change("source_ref", e.target.value)}
                />
              </label>
            </div>
            <label>
              可携带的会话摘要
              <textarea
                value={draft.summary}
                onChange={(e) => change("summary", e.target.value)}
                placeholder="进展和背景，不粘贴秘密或内部原始状态"
              />
            </label>
            <div className="form-actions">
              <button
                disabled={!writable || busy || !draft.name.trim()}
                onClick={() => void action(save)}
              >
                保存会话
              </button>
              <button disabled={busy} onClick={() => choose(selected)}>
                取消编辑
              </button>
            </div>
          </>
        ) : mapDraft ? (
          <>
            <h2>配置本机恢复环境</h2>
            <p className="muted">
              路径、账号引用和确认状态只留在这台设备。保存不登录账号、不读取历史、不运行
              agent。
            </p>
            <label>
              本机 agent 可执行路径
              <input
                value={mapDraft.executable}
                onChange={(e) => updateMap("executable", e.target.value)}
              />
            </label>
            <label>
              本机工作目录
              <input
                value={mapDraft.cwd}
                onChange={(e) => updateMap("cwd", e.target.value)}
              />
            </label>
            <label>
              本机账号引用
              <input
                value={mapDraft.account_ref ?? ""}
                placeholder="credential:reference"
                onChange={(e) => updateMap("account_ref", e.target.value)}
              />
            </label>
            <label>
              运行时版本记录
              <input
                value={mapDraft.runtime_version}
                onChange={(e) => updateMap("runtime_version", e.target.value)}
              />
            </label>
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={mapDraft.state_present_confirmed}
                onChange={(e) =>
                  updateMap("state_present_confirmed", e.target.checked)
                }
              />
              我已确认这台设备和所用账号下存在这个原生会话
            </label>
            <div className="form-actions">
              <button
                disabled={!writable || !admin || busy}
                onClick={() => void action(saveMap)}
              >
                保存本机环境
              </button>
              <button disabled={busy} onClick={() => choose(selected)}>
                取消环境编辑
              </button>
            </div>
          </>
        ) : current?.conflicted ? (
          <>
            <h2>会话存在并发版本</h2>
            <p>核对各版本再整理，保存会引用全部当前版本。</p>
            {current.heads.map((h) => (
              <article key={h.revision}>
                <h3>
                  {h.name}
                  {h.deleted ? "（删除版本）" : ""}
                </h3>
                <pre>{JSON.stringify(h.data, null, 2)}</pre>
                <button
                  disabled={!writable || busy}
                  onClick={() => choose(current.id, draftOf(h, current))}
                >
                  以此版本整理
                </button>
              </article>
            ))}
          </>
        ) : current && inspection ? (
          <>
            <div className="detail-title">
              <h2>{current.heads[0].name}</h2>
              <button
                disabled={!writable || busy || !ready}
                onClick={() =>
                  choose(current.id, draftOf(current.heads[0], current))
                }
              >
                编辑会话关联
              </button>
              <button
                disabled={busy}
                onClick={() =>
                  void action(async () => {
                    setInspection(
                      await call<Inspection>("session.inspect", {
                        id: current.id,
                      }),
                    );
                    setPacket(null);
                    setPlan(null);
                  })
                }
              >
                刷新会话检查
              </button>
            </div>
            <p className="muted">{String(inspection.profile.summary ?? "")}</p>
            <div className="mcp-status-grid">
              <article>
                <small>记录状态</small>
                <strong>{states[String(inspection.profile.status)]}</strong>
                <span>用户记录，不代表进程正在运行</span>
              </article>
              <article>
                <small>原生恢复</small>
                <strong>
                  {inspection.resume.ready
                    ? "可生成本机计划"
                    : "本机条件待处理"}
                </strong>
                <span>存在性由用户确认，登录未核验</span>
              </article>
              <article>
                <small>跨 agent 接续</small>
                <strong>
                  {inspection.continuation.ready
                    ? "材料已具备"
                    : "关联材料待完善"}
                </strong>
                <span>任务和摘要，内部状态不迁移</span>
              </article>
              <article>
                <small>本机环境</small>
                <strong>
                  {inspection.mapping && !inspection.mapping.cleared
                    ? "已登记"
                    : "未登记"}
                </strong>
                <span>账号与路径独立于身份指引</span>
              </article>
            </div>
            <dl className="metadata">
              <dt>来源 agent / 原生 ID</dt>
              <dd>
                {String(inspection.profile.agent || "未指定")} /{" "}
                {String(inspection.profile.native_session_id ?? "未填写")}
              </dd>
              <dt>来源设备</dt>
              <dd>{String(inspection.profile.origin_device_id ?? "未记录")}</dd>
              <dt>环境与来源引用</dt>
              <dd>
                {String(inspection.profile.environment_ref ?? "未填写")} ·{" "}
                {String(inspection.profile.source_ref ?? "手动登记")}
              </dd>
              <dt>会话版本</dt>
              <dd>{inspection.session.revision}</dd>
            </dl>
            {issues.length > 0 && (
              <div className="notice">
                <ul>
                  {issues.map((i) => (
                    <li key={i.code}>{labels[i.code] ?? i.code}</li>
                  ))}
                </ul>
              </div>
            )}
            <h3>记录状态变化</h3>
            <p className="muted">
              暂停、结束或取消只修改记录，不控制 agent 进程。
            </p>
            <div className="form-grid">
              <label>
                会话新状态
                <select
                  disabled={!writable || busy || !ready}
                  value={transition}
                  onChange={(e) => setTransition(e.target.value)}
                >
                  <option value="">选择状态</option>
                  {inspection.allowed_transitions.map((s) => (
                    <option key={s} value={s}>
                      {states[s]}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                会话变化原因
                <textarea
                  disabled={!writable || busy}
                  value={reason}
                  onChange={(e) => setReason(e.target.value)}
                />
              </label>
            </div>
            <button
              disabled={
                !writable || busy || !ready || !transition || !reason.trim()
              }
              onClick={() =>
                void action(async () => {
                  await call("session.transition", {
                    id: current.id,
                    expected_revision: inspection.session.revision,
                    status: transition,
                    reason,
                  });
                  setReason("");
                })
              }
            >
              记录会话状态
            </button>
            <h3>本机原生恢复</h3>
            <div className="form-actions">
              <button
                disabled={!writable || !admin || busy || !ready}
                onClick={editMap}
              >
                配置本机环境
              </button>
              <button
                disabled={busy || !ready || !inspection.resume.ready}
                onClick={() => void action(() => contextPlan(true))}
              >
                生成原生恢复计划
              </button>
              <button
                disabled={
                  !writable ||
                  !admin ||
                  busy ||
                  !ready ||
                  !inspection.mapping ||
                  inspection.mapping.cleared
                }
                onClick={() => {
                  if (
                    window.confirm(
                      "清除本机恢复环境？会话记录和原生状态会保留。",
                    )
                  )
                    void action(async () => {
                      await call("session.clear_mapping", {
                        id: current.id,
                        expected_mapping_revision: inspection.mapping?.revision,
                      });
                      setInspection(
                        await call("session.inspect", { id: current.id }),
                      );
                      setPlan(null);
                    });
                }}
              >
                清除本机环境
              </button>
            </div>
            <p className="muted">
              只生成恢复参数，不执行、不重写原会话的身份指引，不证明登录或历史内容已核验。
            </p>
            <h3>跨 agent 接续材料</h3>
            <div className="form-grid">
              <label>
                接续目标 agent
                <select
                  value={target}
                  onChange={(e) => {
                    setTarget(e.target.value);
                    setPacket(null);
                    setPlan(null);
                  }}
                >
                  {adapters.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                接续设备工作目录
                <input
                  value={cwd}
                  onChange={(e) => {
                    setCwd(e.target.value);
                    setPlan(null);
                  }}
                  placeholder="这台设备的绝对路径"
                />
              </label>
            </div>
            <div className="form-actions">
              <button
                disabled={busy || !ready || !inspection.continuation.ready}
                onClick={() => void action(() => contextPlan())}
              >
                生成会话接续材料
              </button>
              <button
                disabled={
                  busy || !ready || !inspection.continuation.ready || !cwd
                }
                onClick={() => void action(() => contextPlan(false, true))}
              >
                生成新会话接续计划
              </button>
            </div>
            {packet && (
              <label>
                会话接续指令
                <textarea readOnly value={packet.prompt} />
              </label>
            )}
            {plan != null && (
              <pre aria-label="会话计划">{JSON.stringify(plan, null, 2)}</pre>
            )}
            <details>
              <summary>状态记录与关联快照</summary>
              <pre>
                {JSON.stringify(
                  {
                    state_log: inspection.profile.state_log,
                    task: inspection.task_context,
                    identity: inspection.identity_context,
                  },
                  null,
                  2,
                )}
              </pre>
            </details>
            <div className="form-actions">
              {!deleting ? (
                <button
                  className="danger"
                  disabled={!writable || busy || !ready}
                  onClick={() => setDeleting(true)}
                >
                  删除会话记录
                </button>
              ) : (
                <>
                  <span>仅创建删除标记，不删除原生历史。</span>
                  <button
                    className="danger"
                    disabled={!writable || busy || !ready}
                    onClick={() =>
                      void action(async () => {
                        await call("entity.delete", {
                          id: current.id,
                          expected_revision: inspection.session.revision,
                        });
                        choose(null);
                      })
                    }
                  >
                    确认删除会话记录
                  </button>
                  <button onClick={() => setDeleting(false)}>保留记录</button>
                </>
              )}
            </div>
          </>
        ) : (
          <div className="empty">
            <h2>{current ? "正在检查会话" : "让会话与工作关联"}</h2>
            <p>登记原生 ID，关联身份与任务，设备环境单独核对。</p>
          </div>
        )}
      </div>
    </section>
  );
}

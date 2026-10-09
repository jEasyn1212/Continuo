import { useEffect, useState } from "react";
import { Adapter, call, Entity, Event } from "./api";
import { useUnsavedChanges } from "./useUnsavedChanges";

export const stateLabels: Record<string, string> = {
  backlog: "待开始",
  active: "进行中",
  blocked: "受阻",
  paused: "暂停",
  done: "完成",
  cancelled: "取消",
};
interface Entry {
  id: string;
  summary: string;
  transition?: { from: string; to: string; reason: string };
  checks?: string[];
  reason?: string;
  recorded_at_ms: number;
  device_id: string;
}
interface Artifact {
  id: string;
  title: string;
  reference: string;
  verification: string;
  recorded_at_ms: number;
}
interface Profile {
  goal: string;
  status: string;
  success_criteria: string[];
  next_steps: string[];
  blockers: string[];
  identity_id: string | null;
  completion_summary: string;
  progress: Entry[];
  decisions: string[];
  decision_log: Entry[];
  artifact_refs: string[];
  artifacts: Artifact[];
}
interface Inspection {
  task: Event;
  profile: Profile;
  allowed_transitions: string[];
  handoff_ready: boolean;
  issues: { code: string; message: string }[];
  identity_context: { identity: Event } | null;
}
interface Packet {
  task_id: string;
  task_revision: string;
  prompt: string;
  checklist: { id: string; instruction: string }[];
  context: { snapshot: Inspection };
}
interface Draft {
  id?: string;
  revision?: string;
  heads?: string[];
  base: Record<string, unknown>;
  name: string;
  goal: string;
  status: string;
  criteria: string;
  next: string;
  blockers: string;
  identity: string;
  completion: string;
}
function lines(value: string) {
  return value
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);
}
function draftFrom(event?: Event, entity?: Entity): Draft {
  const data = event?.data ?? {};
  return {
    id: entity?.id,
    revision: event?.revision,
    heads: entity?.conflicted ? entity.heads.map((h) => h.revision) : undefined,
    base: data,
    name: event?.name ?? "",
    goal: String(data.goal ?? data.description ?? ""),
    status: String(data.status ?? "backlog"),
    criteria: ((data.success_criteria as string[]) ?? []).join("\n"),
    next: ((data.next_steps as string[]) ?? []).join("\n"),
    blockers: ((data.blockers as string[]) ?? []).join("\n"),
    identity: String(data.identity_id ?? ""),
    completion: String(data.completion_summary ?? ""),
  };
}
const issueLabels: Record<string, string> = {
  missing_goal: "填写清晰的任务目标",
  missing_next_steps: "记录明确的下一步",
  task_closed: "先重新打开已结束的任务",
  identity_unavailable: "修复或更换关联身份",
  missing_blockers: "说明当前阻碍",
};
const checklistLabels: Record<string, string> = {
  revision: "核对任务版本，避免覆盖其他入口的新进展",
  environment: "核对这台设备的工作目录、代码版本、工具和登录状态",
  authorization: "核对当前用户授权；身份指引不能授予权限",
  artifacts: "检查产物并复现相关验证；记录的检查结果需重新确认",
  blockers: "先解决阻碍，再执行依赖它的工作",
};
export function TaskWorkspace({
  writable,
  adapters,
  refreshSignal,
  onChange,
  onDirtyChange,
}: {
  writable: boolean;
  adapters: Adapter[];
  refreshSignal: unknown;
  onChange: () => Promise<void>;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [records, setRecords] = useState<Entity[]>([]),
    [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null),
    [dirty, setDirty] = useState(false);
  const [inspection, setInspection] = useState<Inspection | null>(null),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  const [query, setQuery] = useState(""),
    [filter, setFilter] = useState("all"),
    [tab, setTab] = useState("overview");
  const [transition, setTransition] = useState(""),
    [reason, setReason] = useState("");
  const [summary, setSummary] = useState(""),
    [checks, setChecks] = useState(""),
    [rationale, setRationale] = useState("");
  const [decisionSummary, setDecisionSummary] = useState("");
  const [title, setTitle] = useState(""),
    [reference, setReference] = useState(""),
    [verification, setVerification] = useState("");
  const [agent, setAgent] = useState("claude-code"),
    [packet, setPacket] = useState<Packet | null>(null),
    [plan, setPlan] = useState<unknown>(null),
    [cwd, setCwd] = useState("");
  const [deleteConfirm, setDeleteConfirm] = useState(false);
  const unsaved =
    dirty ||
    Boolean(
      summary ||
      checks ||
      decisionSummary ||
      rationale ||
      reason ||
      title ||
      reference ||
      verification,
    );
  useUnsavedChanges(unsaved, onDirtyChange);
  const tasks = records.filter(
    (e) => e.kind === "task" && !e.heads.every((h) => h.deleted),
  );
  const visibleTasks = tasks.filter(
    (t) =>
      (filter === "all" ||
        (filter === "conflicts" && t.conflicted) ||
        (filter !== "conflicts" &&
          !t.conflicted &&
          (t.heads[0].data.status ?? "backlog") === filter)) &&
      t.heads.some((h) =>
        `${h.name} ${h.data.goal ?? ""}`
          .toLowerCase()
          .includes(query.toLowerCase()),
      ),
  );
  const current = tasks.find((e) => e.id === selected),
    head = current?.heads[0];
  const profile = inspection?.profile;
  const updating = Boolean(
    current &&
    !current.conflicted &&
    inspection?.task.revision !== head?.revision,
  );
  const contextKey = records
    .map((r) => r.heads.map((h) => h.revision).join(","))
    .join(";");
  const identities = records.filter(
    (e) => e.kind === "identity" && !e.conflicted && !e.heads[0].deleted,
  );
  async function load() {
    const result = await call<{ entities: Entity[] }>("entity.list", {
      include_deleted: true,
    });
    setRecords(result.entities);
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
    setInspection((previous) =>
      previous?.task.entity_id === selected && !current?.conflicted
        ? previous
        : null,
    );
    setPacket(null);
    setPlan(null);
    setTransition("");
    if (current && !current.conflicted)
      void call<Inspection>("task.inspect", { id: current.id })
        .then((r) => {
          if (!cancelled) {
            setInspection(r);
            const preferred = r.identity_context?.identity.data.preferred_agent;
            if (
              typeof preferred === "string" &&
              adapters.some((a) => a.id === preferred)
            )
              setAgent(preferred);
          }
        })
        .catch((e) => {
          if (!cancelled) setError(String(e));
        });
    return () => {
      cancelled = true;
    };
  }, [
    selected,
    head?.revision,
    current?.conflicted,
    contextKey,
    refreshSignal,
  ]);
  async function act(fn: () => Promise<void>, mutates = true) {
    setBusy(true);
    setError("");
    try {
      await fn();
      if (mutates) {
        await load();
        await onChange();
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  function select(id: string | null, next: Draft | null = null) {
    if (unsaved && !window.confirm("放弃尚未保存的任务修改和记录？")) return;
    setSelected(id);
    setDraft(next);
    setDirty(false);
    setError("");
    setTab("overview");
    setSummary("");
    setChecks("");
    setRationale("");
    setDecisionSummary("");
    setTitle("");
    setReference("");
    setVerification("");
    setReason("");
    setDeleteConfirm(false);
  }
  function change<K extends keyof Draft>(key: K, value: Draft[K]) {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
    setDirty(true);
  }
  async function save() {
    if (!draft) return;
    const data = {
      ...draft.base,
      goal: draft.goal,
      status: draft.status,
      success_criteria: lines(draft.criteria),
      next_steps: lines(draft.next),
      blockers: lines(draft.blockers),
      identity_id: draft.identity || null,
      completion_summary: draft.completion,
    };
    const result = await call<Entity>(
      draft.heads
        ? "entity.resolve"
        : draft.id
          ? "entity.update"
          : "entity.create",
      {
        ...(draft.id ? { id: draft.id } : { kind: "task" }),
        name: draft.name.trim(),
        data,
        ...(draft.heads
          ? { expected_heads: draft.heads, deleted: false }
          : draft.id
            ? { expected_revision: draft.revision }
            : {}),
      },
    );
    setSelected(result.id);
    setDraft(null);
    setDirty(false);
  }
  function mergeJournal() {
    if (!draft || !current?.conflicted) return;
    const base = { ...draft.base };
    for (const key of ["progress", "decision_log", "artifacts"]) {
      const entries = new Map<string, Record<string, unknown>>();
      for (const version of current.heads)
        for (const entry of (version.data[key] as
          Record<string, unknown>[] | undefined) ?? []) {
          const id = String(entry.id);
          const old = entries.get(id);
          if (old && JSON.stringify(old) !== JSON.stringify(entry)) {
            setError(
              "同一日志记录存在不同内容，请通过接口明确合并；当前草稿保持不变。",
            );
            return;
          }
          entries.set(id, entry);
        }
      base[key] = [...entries.values()];
    }
    for (const key of ["decisions", "artifact_refs"])
      base[key] = [
        ...new Set(
          current.heads.flatMap(
            (h) => (h.data[key] as string[] | undefined) ?? [],
          ),
        ),
      ];
    change("base", base);
  }
  async function record(method: string, params: Record<string, unknown>) {
    if (!current || !head) return;
    await call(method, {
      id: current.id,
      expected_revision: head.revision,
      ...params,
    });
  }
  async function generatePacket() {
    if (!current || !head) return;
    setPacket(
      await call<Packet>("task.handoff", {
        task_id: current.id,
        target_agent: agent,
        expected_revision: head.revision,
      }),
    );
    setPlan(null);
  }
  return (
    <>
      <div className="page-heading">
        <div>
          <div className="eyebrow">WORKSPACE / 02</div>
          <h1>任务</h1>
          <p>把目标、进展与判断留下来，让下一次继续有据可查。</p>
        </div>
        <button
          className="primary"
          disabled={!writable || busy}
          onClick={() => select(null, draftFrom())}
        >
          ＋ 新建任务
        </button>
      </div>
      {error && (
        <div className="notice error" role="alert">
          {error}
        </div>
      )}
      <div className="task-states">
        {Object.entries(stateLabels).map(([state, label]) => (
          <button
            key={state}
            className={filter === state ? "selected" : ""}
            onClick={() => setFilter(filter === state ? "all" : state)}
          >
            <span className={`task-dot ${state}`} />
            {label}
            <strong>
              {
                tasks.filter(
                  (t) =>
                    !t.conflicted &&
                    (t.heads[0].data.status ?? "backlog") === state,
                ).length
              }
            </strong>
          </button>
        ))}
      </div>
      <div className="entity-workspace">
        <section className="entity-list">
          <h2>
            {filter === "all"
              ? "全部任务"
              : filter === "conflicts"
                ? "待合并任务"
                : stateLabels[filter]}
            <span>{visibleTasks.length}</span>
          </h2>
          <input
            aria-label="搜索任务"
            placeholder="搜索目标或任务名称…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {filter !== "all" && (
            <button className="filter-reset" onClick={() => setFilter("all")}>
              查看全部状态
            </button>
          )}
          {tasks.some((t) => t.conflicted) && (
            <button
              className="filter-reset"
              onClick={() => setFilter("conflicts")}
            >
              只看冲突（{tasks.filter((t) => t.conflicted).length}）
            </button>
          )}
          {tasks.length > 0 && visibleTasks.length === 0 && (
            <p className="help">没有符合条件的任务。</p>
          )}
          {tasks.length === 0 && (
            <div className="empty">
              <span>◇</span>
              <h3>从一个明确的目标开始</h3>
              <p>记录验收标准和下一步，之后持续补充进展。</p>
            </div>
          )}
          {visibleTasks.map((t) => (
            <button
              key={t.id}
              className={selected === t.id ? "entity selected" : "entity"}
              disabled={busy}
              onClick={() => select(t.id)}
            >
              <strong>{t.heads[0].name}</strong>
              <small>
                {t.conflicted
                  ? "需要合并版本"
                  : stateLabels[String(t.heads[0].data.status ?? "backlog")]}
              </small>
            </button>
          ))}
        </section>
        <section className="detail">
          {draft ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void act(save);
              }}
            >
              <fieldset className="identity-editor" disabled={busy}>
                <h2>
                  {draft.heads
                    ? "编辑并合并任务"
                    : draft.id
                      ? "编辑任务目标"
                      : "新建任务"}
                </h2>
                {draft.heads && (
                  <div className="notice">
                    从选中版本开始整理，保存会引用全部冲突版本。其他版本保留在历史中。
                    <button type="button" onClick={mergeJournal}>
                      合并各版本新增日志和产物
                    </button>
                  </div>
                )}
                <label>
                  任务名称
                  <input
                    required
                    maxLength={200}
                    value={draft.name}
                    onChange={(e) => change("name", e.target.value)}
                  />
                </label>
                <label>
                  任务目标
                  <textarea
                    rows={4}
                    placeholder="最终希望得到什么结果？"
                    value={draft.goal}
                    onChange={(e) => change("goal", e.target.value)}
                  />
                </label>
                <label>
                  验收标准
                  <textarea
                    rows={3}
                    placeholder="每行一条，描述怎样确认工作完成"
                    value={draft.criteria}
                    onChange={(e) => change("criteria", e.target.value)}
                  />
                </label>
                <label>
                  下一步
                  <textarea
                    rows={3}
                    placeholder="每行一个可执行步骤"
                    value={draft.next}
                    onChange={(e) => change("next", e.target.value)}
                  />
                </label>
                <label>
                  关联身份
                  <select
                    value={draft.identity}
                    onChange={(e) => change("identity", e.target.value)}
                  >
                    <option value="">不指定，准备计划时使用本机当前身份</option>
                    {identities.map((i) => (
                      <option key={i.id} value={i.id}>
                        {i.heads[0].name}
                      </option>
                    ))}
                    {draft.identity &&
                      !identities.some((i) => i.id === draft.identity) && (
                        <option value={draft.identity}>
                          身份不可用，请选择其他身份或清除
                        </option>
                      )}
                  </select>
                </label>
                {draft.heads && (
                  <label>
                    合并后的任务状态
                    <select
                      value={draft.status}
                      onChange={(e) => change("status", e.target.value)}
                    >
                      {Object.entries(stateLabels).map(([value, label]) => (
                        <option key={value} value={value}>
                          {label}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
                <label>
                  当前阻碍
                  <textarea
                    rows={2}
                    placeholder="每行一项；受阻状态必须填写"
                    value={draft.blockers}
                    onChange={(e) => change("blockers", e.target.value)}
                  />
                </label>
                {draft.status === "done" && (
                  <label>
                    完成结论
                    <textarea
                      required
                      rows={3}
                      value={draft.completion}
                      onChange={(e) => change("completion", e.target.value)}
                    />
                  </label>
                )}
                <p className="help">
                  填写可携带的工作记录。文件位置以
                  project:相对路径引用；账号、密钥和本机绝对路径分别管理。
                </p>
                <div className="form-actions">
                  <button type="button" onClick={() => select(selected)}>
                    取消
                  </button>
                  <button
                    className="primary"
                    disabled={!writable || busy || !draft.name.trim()}
                  >
                    保存任务
                  </button>
                </div>
              </fieldset>
            </form>
          ) : current && head ? (
            <>
              <div className="detail-title">
                <div>
                  <span className="pill">
                    {current.conflicted
                      ? "版本冲突"
                      : stateLabels[String(head.data.status ?? "backlog")]}
                  </span>
                  <h2>{head.name}</h2>
                </div>
                {!current.conflicted && (
                  <button
                    disabled={!writable || busy}
                    onClick={() => {
                      setDraft(draftFrom(head, current));
                      setDirty(false);
                    }}
                  >
                    编辑目标
                  </button>
                )}
              </div>
              {current.conflicted ? (
                <>
                  <div className="notice">
                    并发修改保留了多个版本。合并前不能记录新进展或生成接续材料。
                  </div>
                  {current.heads.map((h, i) => (
                    <article className="version" key={h.revision}>
                      <h3>
                        版本 {i + 1} · {h.name}
                      </h3>
                      <small>
                        {new Date(h.timestamp_ms).toLocaleString()}
                        {h.deleted ? " · 删除版本" : ""}
                      </small>
                      <p className="body-text">
                        {String(h.data.goal ?? h.data.description ?? "")}
                      </p>
                      <details>
                        <summary>查看这个版本全部内容</summary>
                        <pre>{JSON.stringify(h.data, null, 2)}</pre>
                      </details>
                      <button
                        disabled={!writable || busy}
                        onClick={() => setDraft(draftFrom(h, current))}
                      >
                        以此版本编辑合并
                      </button>
                    </article>
                  ))}
                </>
              ) : inspection && profile ? (
                <fieldset
                  className="identity-editor"
                  disabled={busy || updating}
                >
                  {updating && (
                    <p className="help" role="status">
                      正在读取新版本…
                    </p>
                  )}
                  <nav className="task-tabs" aria-label="任务详情">
                    {[
                      ["overview", "目标与状态"],
                      ["progress", "进展"],
                      ["decisions", "决策"],
                      ["artifacts", "产物"],
                      ["handoff", "接续"],
                    ].map(([value, label]) => (
                      <button
                        className={tab === value ? "selected" : ""}
                        key={value}
                        onClick={() => {
                          setTab(value);
                        }}
                      >
                        {label}
                      </button>
                    ))}
                  </nav>
                  {tab === "overview" && (
                    <>
                      <h3>目标</h3>
                      <p className="body-text">
                        {profile.goal || "尚未填写目标"}
                      </p>
                      <h3>验收标准</h3>
                      {profile.success_criteria.length ? (
                        <ul className="task-lines">
                          {profile.success_criteria.map((c, i) => (
                            <li key={i}>{c}</li>
                          ))}
                        </ul>
                      ) : (
                        <p className="help">
                          尚未明确验收标准，编辑目标后补充。
                        </p>
                      )}
                      <h3>下一步</h3>
                      <ol className="task-lines">
                        {profile.next_steps.map((s, i) => (
                          <li key={i}>{s}</li>
                        ))}
                      </ol>
                      {profile.blockers.length > 0 && (
                        <div className="notice">
                          <strong>当前阻碍</strong>
                          <ul>
                            {profile.blockers.map((b, i) => (
                              <li key={i}>{b}</li>
                            ))}
                          </ul>
                        </div>
                      )}
                      <p className="help">
                        关联身份：
                        {inspection.identity_context?.identity.name ??
                          (profile.identity_id
                            ? "不可用"
                            : "准备计划时使用本机当前身份")}
                      </p>
                      {profile.completion_summary && (
                        <div className="identity-check">
                          <h3>
                            {profile.status === "done"
                              ? "完成结论"
                              : "上次完成记录"}
                          </h3>
                          <p className="body-text">
                            {profile.completion_summary}
                          </p>
                        </div>
                      )}
                      <form
                        className="task-record-form"
                        onSubmit={(e) => {
                          e.preventDefault();
                          void act(async () => {
                            await record("task.transition", {
                              status: transition,
                              reason,
                            });
                            setReason("");
                            setTransition("");
                          });
                        }}
                      >
                        <h3>改变任务状态</h3>
                        <label>
                          新状态
                          <select
                            required
                            value={transition}
                            disabled={!writable || busy}
                            onChange={(e) => setTransition(e.target.value)}
                          >
                            <option value="">选择允许的状态…</option>
                            {inspection.allowed_transitions.map((s) => (
                              <option key={s} value={s}>
                                {stateLabels[s]}
                                {profile.status === "done" ||
                                profile.status === "cancelled"
                                  ? " · 重新打开"
                                  : ""}
                              </option>
                            ))}
                          </select>
                        </label>
                        <label>
                          {transition === "done"
                            ? "完成结论"
                            : transition === "blocked"
                              ? "阻碍说明"
                              : "变化原因"}
                          <textarea
                            required
                            rows={3}
                            value={reason}
                            disabled={!writable || busy}
                            onChange={(e) => setReason(e.target.value)}
                          />
                        </label>
                        <button
                          disabled={
                            !writable || busy || !transition || !reason.trim()
                          }
                        >
                          记录状态变化
                        </button>
                      </form>
                    </>
                  )}
                  {tab === "progress" && (
                    <>
                      <h3>
                        进展记录 <small>{profile.progress.length}</small>
                      </h3>
                      {profile.progress.length === 0 && (
                        <p className="help">
                          记录已经完成的工作，以及你实际执行过的检查。
                        </p>
                      )}
                      {profile.progress.map((p) => (
                        <article className="task-entry" key={p.id}>
                          <small>
                            {new Date(p.recorded_at_ms).toLocaleString()} · 设备{" "}
                            {p.device_id.slice(0, 8)}
                          </small>
                          <p className="body-text">
                            {p.transition
                              ? `${stateLabels[p.transition.from]} → ${stateLabels[p.transition.to]}：${p.transition.reason}`
                              : p.summary}
                          </p>
                          {p.checks?.length ? (
                            <>
                              <strong>记录的检查</strong>
                              <ul className="task-lines">
                                {p.checks.map((c, i) => (
                                  <li key={i}>{c}</li>
                                ))}
                              </ul>
                            </>
                          ) : null}
                        </article>
                      ))}
                      <form
                        className="task-record-form"
                        onSubmit={(e) => {
                          e.preventDefault();
                          void act(async () => {
                            await record("task.progress", {
                              summary,
                              checks: lines(checks),
                            });
                            setSummary("");
                            setChecks("");
                          });
                        }}
                      >
                        <label>
                          本次进展
                          <textarea
                            required
                            rows={3}
                            value={summary}
                            disabled={!writable || busy}
                            onChange={(e) => setSummary(e.target.value)}
                          />
                        </label>
                        <label>
                          已执行的检查
                          <textarea
                            rows={2}
                            placeholder="每行一条检查及实际结果；未执行就留空"
                            value={checks}
                            disabled={!writable || busy}
                            onChange={(e) => setChecks(e.target.value)}
                          />
                        </label>
                        <button disabled={!writable || busy || !summary.trim()}>
                          记录进展
                        </button>
                      </form>
                    </>
                  )}
                  {tab === "decisions" && (
                    <>
                      <h3>决策与理由</h3>
                      {profile.decisions.map((d, i) => (
                        <article className="task-entry" key={i}>
                          <p className="body-text">{d}</p>
                        </article>
                      ))}
                      {profile.decision_log.map((d) => (
                        <article className="task-entry" key={d.id}>
                          <h4>{d.summary}</h4>
                          <p className="body-text">{d.reason}</p>
                          <small>
                            {new Date(d.recorded_at_ms).toLocaleString()}
                          </small>
                        </article>
                      ))}
                      <form
                        className="task-record-form"
                        onSubmit={(e) => {
                          e.preventDefault();
                          void act(async () => {
                            await record("task.decision", {
                              summary: decisionSummary,
                              reason: rationale,
                            });
                            setDecisionSummary("");
                            setRationale("");
                          });
                        }}
                      >
                        <label>
                          决策
                          <textarea
                            required
                            rows={2}
                            value={decisionSummary}
                            disabled={!writable || busy}
                            onChange={(e) => setDecisionSummary(e.target.value)}
                          />
                        </label>
                        <label>
                          理由与权衡
                          <textarea
                            required
                            rows={3}
                            value={rationale}
                            disabled={!writable || busy}
                            onChange={(e) => setRationale(e.target.value)}
                          />
                        </label>
                        <button
                          disabled={
                            !writable ||
                            busy ||
                            !decisionSummary.trim() ||
                            !rationale.trim()
                          }
                        >
                          记录决策
                        </button>
                      </form>
                    </>
                  )}
                  {tab === "artifacts" && (
                    <>
                      <h3>产物与证据</h3>
                      <p className="help">
                        这里只登记引用，不上传文件。验证说明由记录者提供，接续时应重新检查。
                      </p>
                      {profile.artifact_refs.map((r, i) => (
                        <article className="task-entry" key={i}>
                          <code>{r}</code>
                        </article>
                      ))}
                      {profile.artifacts.map((a) => (
                        <article className="task-entry" key={a.id}>
                          <h4>{a.title}</h4>
                          <code className="artifact-reference">
                            {a.reference}
                          </code>
                          <p className="body-text">
                            {a.verification || "尚未记录验证说明"}
                          </p>
                        </article>
                      ))}
                      <form
                        className="task-record-form"
                        onSubmit={(e) => {
                          e.preventDefault();
                          void act(async () => {
                            await record("task.artifact", {
                              title,
                              reference,
                              ...(verification ? { verification } : {}),
                            });
                            setTitle("");
                            setReference("");
                            setVerification("");
                          });
                        }}
                      >
                        <label>
                          产物名称
                          <input
                            required
                            value={title}
                            disabled={!writable || busy}
                            onChange={(e) => setTitle(e.target.value)}
                          />
                        </label>
                        <label>
                          可携带的引用
                          <input
                            required
                            placeholder="project:docs/report.md 或无凭据的 HTTPS 地址"
                            value={reference}
                            disabled={!writable || busy}
                            onChange={(e) => setReference(e.target.value)}
                          />
                        </label>
                        <label>
                          验证说明
                          <textarea
                            rows={3}
                            value={verification}
                            disabled={!writable || busy}
                            onChange={(e) => setVerification(e.target.value)}
                          />
                        </label>
                        <button
                          disabled={
                            !writable ||
                            busy ||
                            !title.trim() ||
                            !reference.trim()
                          }
                        >
                          登记产物
                        </button>
                      </form>
                    </>
                  )}
                  {tab === "handoff" && (
                    <>
                      <h3>准备下一次接续</h3>
                      {!inspection.handoff_ready && (
                        <div className="notice">
                          先补齐接续所需内容：
                          <ul>
                            {inspection.issues.map((i) => (
                              <li key={i.code}>
                                {issueLabels[i.code] ?? i.message}
                              </li>
                            ))}
                          </ul>
                        </div>
                      )}
                      <p className="help">
                        材料包含当前任务与身份版本、目标、进展、决策、产物及下一步，不迁移
                        agent 内部状态。
                      </p>
                      <label>
                        目标 agent
                        <select
                          value={agent}
                          disabled={busy}
                          onChange={(e) => {
                            setAgent(e.target.value);
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
                      <button
                        className="primary"
                        disabled={busy || !inspection.handoff_ready}
                        onClick={() => void act(generatePacket, false)}
                      >
                        生成接续材料
                      </button>
                      {packet && (
                        <div className="handoff-packet">
                          <h3>接续前检查</h3>
                          <ol className="task-lines">
                            {packet.checklist.map((c) => (
                              <li key={c.id}>
                                {checklistLabels[c.id] ?? c.instruction}
                              </li>
                            ))}
                          </ol>
                          <label>
                            可复制的任务指令
                            <textarea
                              readOnly
                              rows={12}
                              value={packet.prompt}
                              aria-label="接续指令"
                            />
                          </label>
                          <details>
                            <summary>版本与结构化材料</summary>
                            <pre>{JSON.stringify(packet, null, 2)}</pre>
                          </details>
                          <form
                            className="task-record-form"
                            onSubmit={(e) => {
                              e.preventDefault();
                              void act(async () => {
                                setPlan(
                                  await call("agent.prepare", {
                                    agent,
                                    cwd,
                                    task_id: current.id,
                                    expected_task_revision:
                                      packet.task_revision,
                                  }),
                                );
                              }, false);
                            }}
                          >
                            <label>
                              目标设备工作目录
                              <input
                                required
                                placeholder="这台设备的绝对路径"
                                value={cwd}
                                disabled={busy}
                                onChange={(e) => {
                                  setCwd(e.target.value);
                                  setPlan(null);
                                }}
                              />
                            </label>
                            <button disabled={busy || !cwd.trim()}>
                              生成使用此任务的启动计划
                            </button>
                            <p className="help">
                              生成计划前重新核对任务版本；当前不启动 agent
                              或改写原生配置。
                            </p>
                          </form>
                          {plan !== null && (
                            <pre aria-label="任务启动计划">
                              {JSON.stringify(plan, null, 2)}
                            </pre>
                          )}
                        </div>
                      )}
                    </>
                  )}
                  <details className="task-raw">
                    <summary>完整记录与版本</summary>
                    <pre>{JSON.stringify(current, null, 2)}</pre>
                  </details>
                  <div className="identity-delete">
                    {deleteConfirm ? (
                      <>
                        <p>删除会同步并保留历史。</p>
                        <button
                          disabled={busy}
                          onClick={() => setDeleteConfirm(false)}
                        >
                          取消
                        </button>{" "}
                        <button
                          className="danger"
                          disabled={!writable || busy}
                          onClick={() =>
                            void act(async () => {
                              await record("entity.delete", {});
                              setSelected(null);
                              setDeleteConfirm(false);
                            })
                          }
                        >
                          确认删除
                        </button>
                      </>
                    ) : (
                      <button
                        disabled={!writable || busy}
                        onClick={() => setDeleteConfirm(true)}
                      >
                        删除任务…
                      </button>
                    )}
                  </div>
                </fieldset>
              ) : (
                <p className="help">正在读取任务状态…</p>
              )}
            </>
          ) : (
            <div className="empty">
              <span>↖</span>
              <h3>选择一个任务</h3>
              <p>查看目标、记录工作，或者为另一个 agent 准备接续材料。</p>
            </div>
          )}
        </section>
      </div>
    </>
  );
}

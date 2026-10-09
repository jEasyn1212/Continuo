import { useEffect, useState } from "react";
import { Adapter, call, Entity, Event } from "./api";
import { useUnsavedChanges } from "./useUnsavedChanges";
interface Profile {
  capability_type: string;
  description: string;
  body: string;
  version: string;
  source_ref: string | null;
  source_revision: string | null;
  source_license: string | null;
  agent_targets: string[];
  requires: string[];
  reviewed_digest: string | null;
}
interface Inspection {
  capability: Event;
  profile: Profile;
  digest: string;
  ready: boolean;
  issues: { id: string; code: string }[];
  resolved: Event[];
}
interface Draft {
  id?: string;
  revision?: string;
  heads?: string[];
  base: Record<string, unknown>;
  name: string;
  capability_type: string;
  description: string;
  body: string;
  version: string;
  source_ref: string;
  source_revision: string;
  source_license: string;
  agent_targets: string[];
  requires: string[];
}
function draftOf(event?: Event, record?: Entity): Draft {
  const p = event?.data ?? {};
  return {
    id: record?.id,
    revision: event?.revision,
    heads: record?.conflicted ? record.heads.map((h) => h.revision) : undefined,
    base: p,
    name: event?.name ?? "",
    capability_type: String(p.capability_type ?? "rule"),
    description: String(p.description ?? ""),
    body: String(p.body ?? ""),
    version: String(p.version ?? "draft"),
    source_ref: String(p.source_ref ?? ""),
    source_revision: String(p.source_revision ?? ""),
    source_license: String(p.source_license ?? ""),
    agent_targets: (p.agent_targets as string[]) ?? [],
    requires: (p.requires as string[]) ?? [],
  };
}
const issueNames: Record<string, string> = {
  body_missing: "尚未填写正文",
  review_required: "正文或元信息尚待检查",
  not_found: "依赖不存在",
  capability_unavailable: "记录已删除、类型错误或存在冲突",
  dependency_cycle: "依赖形成循环",
  agent_incompatible: "不适用于当前 agent",
};
export function CapabilityWorkspace({
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
  const [records, setRecords] = useState<Entity[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [dirty, setDirty] = useState(false);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [agent, setAgent] = useState("claude-code");
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [cwd, setCwd] = useState("");
  const [plan, setPlan] = useState<unknown>(null);
  const [deleteConfirm, setDeleteConfirm] = useState(false);
  useUnsavedChanges(dirty, onDirtyChange);
  const current = records.find((r) => r.id === selected);
  const revisionKey = current?.heads.map((h) => h.revision).join(",");
  async function load() {
    const all = await call<{ entities: Entity[] }>("entity.list", {
      kind: "capability",
    });
    setRecords(all.entities);
  }
  useEffect(() => {
    let cancelled = false;
    void call<{ entities: Entity[] }>("entity.list", { kind: "capability" })
      .then((all) => {
        if (!cancelled) setRecords(all.entities);
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
    setPlan(null);
    setDeleteConfirm(false);
    if (!selected || !current || current.conflicted) {
      setInspection(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    void call<Inspection>("capability.inspect", {
      id: selected,
      target_agent: agent,
    })
      .then((value) => {
        if (!cancelled) setInspection(value);
      })
      .catch((e) => {
        if (!cancelled) {
          setInspection(null);
          setError(String(e));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selected, revisionKey, agent]);
  async function act(fn: () => Promise<void>) {
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
  function choose(id: string | null, next: Draft | null = null) {
    if (dirty && !window.confirm("放弃尚未保存的能力修改？")) return;
    setSelected(id);
    setDraft(next);
    setDirty(false);
    setError("");
    if (id !== selected) setInspection(null);
    setPlan(null);
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
      capability_type: draft.capability_type,
      description: draft.description,
      body: draft.body,
      version: draft.version,
      source_ref: draft.source_ref || null,
      source_revision: draft.source_revision || null,
      source_license: draft.source_license || null,
      agent_targets: draft.agent_targets,
      requires: draft.requires,
    };
    const result = await call<Entity>(
      draft.heads
        ? "entity.resolve"
        : draft.id
          ? "entity.update"
          : "entity.create",
      draft.heads
        ? { id: draft.id, expected_heads: draft.heads, name: draft.name, data }
        : draft.id
          ? {
              id: draft.id,
              expected_revision: draft.revision,
              name: draft.name,
              data,
            }
          : { kind: "capability", name: draft.name, data },
    );
    setDraft(null);
    setDirty(false);
    setSelected(result.id);
    setInspection(null);
    setPlan(null);
  }
  function toggle(field: "agent_targets" | "requires", id: string) {
    if (draft)
      change(
        field,
        draft[field].includes(id)
          ? draft[field].filter((v) => v !== id)
          : [...draft[field], id],
      );
  }
  const shown = records.filter(
    (r) =>
      (filter === "all" ||
        (filter === "conflict"
          ? r.conflicted
          : r.heads.some(
              (h) => (h.data.capability_type ?? "rule") === filter,
            ))) &&
      r.heads.some((h) =>
        `${h.name} ${h.data.description ?? ""}`
          .toLowerCase()
          .includes(query.toLowerCase()),
      ),
  );
  const canAct =
    !busy &&
    !loading &&
    inspection?.capability.revision === current?.heads[0]?.revision;
  return (
    <>
      <div className="page-heading">
        <div>
          <div className="eyebrow">WORKSPACE / 03</div>
          <h1>能力</h1>
          <p>把规则和 skill 的正文、来源与版本带到不同设备和 agent。</p>
        </div>
        <div className="capability-actions">
          <button
            disabled={!writable || busy}
            onClick={() =>
              choose(null, { ...draftOf(), capability_type: "skill" })
            }
          >
            导入文本
          </button>
          <button
            className="primary"
            disabled={!writable || busy}
            onClick={() => choose(null, draftOf())}
          >
            ＋ 新建能力
          </button>
        </div>
      </div>
      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      {!writable && (
        <div className="notice">当前为只读入口，可以检查适用性并生成计划。</div>
      )}
      <div className="entity-workspace">
        <section className="entity-list">
          <h2>
            能力库<span>{records.length}</span>
          </h2>
          <input
            className="identity-search"
            aria-label="搜索能力"
            placeholder="搜索名称或说明"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <label>
            能力筛选
            <select value={filter} onChange={(e) => setFilter(e.target.value)}>
              <option value="all">全部</option>
              <option value="rule">规则</option>
              <option value="skill">Skill</option>
              <option value="conflict">存在冲突</option>
            </select>
          </label>
          {shown.length === 0 && (
            <div className="empty">
              <span>◇</span>
              <h3>{records.length ? "没有匹配的能力" : "从一条规则开始"}</h3>
              <p>填写正文、检查内容，再绑定身份或生成 agent 计划。</p>
            </div>
          )}
          {shown.map((r) => (
            <button
              className={selected === r.id ? "entity selected" : "entity"}
              disabled={busy}
              key={r.id}
              onClick={() => choose(r.id)}
            >
              <strong>{r.heads[0].name}</strong>
              <small>
                {r.conflicted
                  ? "需要解决冲突"
                  : `${r.heads[0].data.capability_type === "skill" ? "Skill" : "规则"} · ${String(r.heads[0].data.version ?? "draft")}`}
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
              <fieldset
                className="identity-editor"
                disabled={busy || !writable}
              >
                <h2>
                  {draft.heads
                    ? "整理冲突版本"
                    : draft.id
                      ? "编辑能力"
                      : "新建或导入能力"}
                </h2>
                <label>
                  能力名称
                  <input
                    required
                    maxLength={200}
                    value={draft.name}
                    onChange={(e) => change("name", e.target.value)}
                  />
                </label>
                <div className="capability-pair">
                  <label>
                    能力类型
                    <select
                      value={draft.capability_type}
                      onChange={(e) =>
                        change("capability_type", e.target.value)
                      }
                    >
                      <option value="rule">规则</option>
                      <option value="skill">Skill（文本）</option>
                    </select>
                  </label>
                  <label>
                    内容版本
                    <input
                      required
                      maxLength={256}
                      value={draft.version}
                      onChange={(e) => change("version", e.target.value)}
                    />
                  </label>
                </div>
                <label>
                  能力说明
                  <textarea
                    rows={2}
                    maxLength={8192}
                    value={draft.description}
                    onChange={(e) => change("description", e.target.value)}
                  />
                </label>
                <label>
                  能力正文
                  <textarea
                    className="capability-body"
                    rows={13}
                    maxLength={65536}
                    placeholder="粘贴 Markdown 规则或 SKILL.md 的文本内容"
                    value={draft.body}
                    onChange={(e) => change("body", e.target.value)}
                  />
                </label>
                <p className="help">
                  只保存文本。不会读取来源地址、安装目录资源或执行脚本；正文中不要包含密钥和本机绝对路径。
                </p>
                <label>
                  来源引用
                  <input
                    placeholder="https://… 或 project:skills/example/SKILL.md"
                    value={draft.source_ref}
                    onChange={(e) => change("source_ref", e.target.value)}
                  />
                </label>
                <div className="capability-pair">
                  <label>
                    来源版本或 commit
                    <input
                      maxLength={1024}
                      value={draft.source_revision}
                      onChange={(e) =>
                        change("source_revision", e.target.value)
                      }
                    />
                  </label>
                  <label>
                    来源许可
                    <input
                      maxLength={1024}
                      placeholder="例如 MIT；以来源实际许可为准"
                      value={draft.source_license}
                      onChange={(e) => change("source_license", e.target.value)}
                    />
                  </label>
                </div>
                <fieldset className="binding-field">
                  <legend>
                    适用 agent <small>不选择表示三个均适用</small>
                  </legend>
                  {adapters.map((a) => (
                    <label className="binding-choice" key={a.id}>
                      <input
                        type="checkbox"
                        checked={draft.agent_targets.includes(a.id)}
                        onChange={() => toggle("agent_targets", a.id)}
                      />
                      <span>{a.name}</span>
                    </label>
                  ))}
                  {draft.agent_targets
                    .filter((id) => !adapters.some((a) => a.id === id))
                    .map((id) => (
                      <label className="binding-choice" key={id}>
                        <input
                          type="checkbox"
                          checked
                          onChange={() => toggle("agent_targets", id)}
                        />
                        <span>
                          未知 agent · {id}
                          <small>请取消或等待产品支持后再使用</small>
                        </span>
                      </label>
                    ))}
                </fieldset>
                <fieldset className="binding-field">
                  <legend>依赖能力</legend>
                  {records
                    .filter((r) => r.id !== draft.id)
                    .map((r) => (
                      <label className="binding-choice" key={r.id}>
                        <input
                          type="checkbox"
                          checked={draft.requires.includes(r.id)}
                          disabled={
                            r.conflicted && !draft.requires.includes(r.id)
                          }
                          onChange={() => toggle("requires", r.id)}
                        />
                        <span>
                          {r.heads[0].name}
                          <small>
                            {r.conflicted
                              ? "先解决冲突"
                              : String(r.heads[0].data.version ?? "draft")}
                          </small>
                        </span>
                      </label>
                    ))}
                  {draft.requires
                    .filter(
                      (id) =>
                        !records.some((r) => r.id === id && r.id !== draft.id),
                    )
                    .map((id) => (
                      <label className="binding-choice" key={id}>
                        <input
                          type="checkbox"
                          checked
                          onChange={() => toggle("requires", id)}
                        />
                        <span>
                          失效依赖 · {id}
                          <small>取消关联以修复</small>
                        </span>
                      </label>
                    ))}
                  {records.filter((r) => r.id !== draft.id).length === 0 && (
                    <p className="help">暂无可关联能力。</p>
                  )}
                </fieldset>
                {draft.heads && (
                  <p className="notice">
                    保存后生成引用全部冲突版本的新版本；旧正文与删除记录保留在历史中。请检查选定正文与其他版本的差异。
                  </p>
                )}
                <div className="capability-actions">
                  <button className="primary">保存能力</button>
                  <button type="button" onClick={() => choose(selected)}>
                    取消编辑
                  </button>
                </div>
              </fieldset>
            </form>
          ) : current ? (
            <>
              <div className="detail-title">
                <h2>{current.heads[0].name}</h2>
                <span className="pill">
                  {current.conflicted
                    ? "并发版本"
                    : current.heads[0].data.capability_type === "skill"
                      ? "Skill"
                      : "规则"}
                </span>
              </div>
              {current.conflicted ? (
                <>
                  <p className="notice">
                    两边内容均已保留。选择一个版本作为整理起点，然后逐项检查并保存。
                  </p>
                  {current.heads.map((h) => (
                    <article className="version" key={h.revision}>
                      <small>
                        {h.deleted
                          ? "删除版本"
                          : String(h.data.version ?? "draft")}{" "}
                        · {h.revision}
                      </small>
                      <h3>{h.name}</h3>
                      <pre className="capability-preview">
                        {String(h.data.body ?? "（无正文）")}
                      </pre>
                      <details>
                        <summary>全部版本字段</summary>
                        <pre>{JSON.stringify(h.data, null, 2)}</pre>
                      </details>
                      <button
                        disabled={!writable || busy}
                        onClick={() => choose(current.id, draftOf(h, current))}
                      >
                        以此版本整理
                      </button>
                    </article>
                  ))}
                </>
              ) : (
                <>
                  <p className="body-text">
                    {String(current.heads[0].data.description ?? "")}
                  </p>
                  <div className="identity-tags">
                    <span>
                      版本 {String(current.heads[0].data.version ?? "draft")}
                    </span>
                    <span>文本上下文</span>
                    <span>权限单独控制</span>
                  </div>
                  <div className="capability-source">
                    <strong>来源与许可</strong>
                    <p>
                      {String(
                        current.heads[0].data.source_ref ??
                          "本地编写 / 来源未声明",
                      )}
                    </p>
                    <small>
                      来源版本：
                      {String(
                        current.heads[0].data.source_revision ?? "未声明",
                      )}{" "}
                      · 许可：
                      {String(current.heads[0].data.source_license ?? "未声明")}
                    </small>
                    <p className="help">来源由导入者提供，尚未远端核验。</p>
                  </div>
                  <h3>正文</h3>
                  <pre className="capability-preview">
                    {String(current.heads[0].data.body ?? "尚未填写正文") ||
                      "尚未填写正文"}
                  </pre>
                  <button
                    disabled={!writable || busy}
                    onClick={() =>
                      choose(current.id, draftOf(current.heads[0], current))
                    }
                  >
                    编辑能力
                  </button>
                  <div className="identity-check">
                    <h3>检查与应用</h3>
                    <label>
                      检查目标 agent
                      <select
                        value={agent}
                        disabled={busy}
                        onChange={(e) => setAgent(e.target.value)}
                      >
                        {adapters.map((a) => (
                          <option key={a.id} value={a.id}>
                            {a.name}
                          </option>
                        ))}
                      </select>
                    </label>
                    {loading ? (
                      <p>正在检查当前版本与依赖…</p>
                    ) : inspection ? (
                      <>
                        <p>
                          {inspection.ready
                            ? "当前正文与依赖已检查，可加入启动计划。"
                            : "需要处理以下事项："}
                        </p>
                        <ul>
                          {inspection.issues.map((i, n) => (
                            <li key={`${i.id}-${i.code}-${n}`}>
                              {issueNames[i.code] ?? i.code} ·{" "}
                              {records.find((r) => r.id === i.id)?.heads[0]
                                .name ?? i.id}
                            </li>
                          ))}
                        </ul>
                        <details>
                          <summary>内容摘要与依赖版本</summary>
                          <p>
                            <code>{inspection.digest}</code>
                          </p>
                          {inspection.resolved.map((e) => (
                            <p key={e.entity_id}>
                              {e.name} · {e.revision}
                            </p>
                          ))}
                        </details>
                        {inspection.issues.some(
                          (i) =>
                            i.id === current.id && i.code === "review_required",
                        ) && (
                          <>
                            <p className="help">
                              请先阅读正文和来源。标记只确认这一版内容，不授予工具、文件或命令执行权限。
                            </p>
                            <button
                              disabled={
                                !writable ||
                                !canAct ||
                                !inspection.profile.body.trim()
                              }
                              onClick={() =>
                                void act(async () => {
                                  await call("capability.review", {
                                    id: current.id,
                                    expected_revision:
                                      inspection.capability.revision,
                                    expected_digest: inspection.digest,
                                  });
                                })
                              }
                            >
                              标记正文已检查
                            </button>
                          </>
                        )}
                        <form
                          onSubmit={(e) => {
                            e.preventDefault();
                            void act(async () => {
                              setPlan(
                                await call("agent.prepare", {
                                  agent,
                                  cwd,
                                  capability_ids: [current.id],
                                  use_current_identity: false,
                                }),
                              );
                            });
                          }}
                        >
                          <label>
                            本机工作目录
                            <input
                              required
                              placeholder="绝对路径"
                              value={cwd}
                              onChange={(e) => {
                                setCwd(e.target.value);
                                setPlan(null);
                              }}
                            />
                          </label>
                          <button
                            className="primary"
                            disabled={!canAct || !inspection.ready}
                          >
                            生成能力启动计划
                          </button>
                        </form>
                      </>
                    ) : (
                      <p>检查尚未完成，请刷新状态重试。</p>
                    )}
                    <p className="help">
                      依赖先于当前能力应用。三个适配器使用各自的文本入口；此步骤只生成参数，不启动
                      agent 或改写配置。
                    </p>
                  </div>
                  {plan != null && (
                    <details open>
                      <summary>能力启动计划</summary>
                      <pre>{JSON.stringify(plan, null, 2)}</pre>
                    </details>
                  )}
                  <div className="identity-delete">
                    {deleteConfirm ? (
                      <>
                        <p>
                          删除会同步为删除记录，已绑定身份或依赖这条能力的计划会被阻止。
                        </p>
                        <button
                          className="danger"
                          disabled={!writable || busy}
                          onClick={() =>
                            void act(async () => {
                              await call("entity.delete", {
                                id: current.id,
                                expected_revision: current.heads[0].revision,
                              });
                              choose(null);
                            })
                          }
                        >
                          确认删除能力
                        </button>
                        <button onClick={() => setDeleteConfirm(false)}>
                          取消
                        </button>
                      </>
                    ) : (
                      <button
                        className="danger"
                        disabled={!writable || busy}
                        onClick={() => setDeleteConfirm(true)}
                      >
                        删除能力
                      </button>
                    )}
                  </div>
                </>
              )}
            </>
          ) : (
            <div className="empty">
              <span>◇</span>
              <h3>选择一条能力</h3>
              <p>正文与因果版本保存在本机，配置加密同步后可以带到其他设备。</p>
            </div>
          )}
        </section>
      </div>
    </>
  );
}

import { useEffect, useState } from "react";
import { Adapter, call, Entity, Event } from "./api";
import { useUnsavedChanges } from "./useUnsavedChanges";

export interface CurrentIdentity {
  selection: { revision: string; identity_id: string | null };
  state: "none" | "ready" | "unavailable";
  context: Inspection | null;
}
interface Inspection {
  identity: Event;
  profile: Record<string, unknown>;
  ready: boolean;
  bindings: { id: string; kind: string; state: string; record?: Entity }[];
}
interface Draft {
  id?: string;
  revision?: string;
  heads?: string[];
  base: Record<string, unknown>;
  name: string;
  description: string;
  instructions: string;
  preferred_agent: string;
  capability_ids: string[];
  mcp_ids: string[];
}
function fromEvent(event?: Event, entity?: Entity): Draft {
  const data = event?.data ?? {};
  return {
    id: entity?.id,
    revision: event?.revision,
    heads: entity?.conflicted ? entity.heads.map((h) => h.revision) : undefined,
    base: data,
    name: event?.name ?? "",
    description: String(data.description ?? ""),
    instructions: String(data.instructions ?? ""),
    preferred_agent: String(data.preferred_agent ?? ""),
    capability_ids: (data.capability_ids as string[] | undefined) ?? [],
    mcp_ids: (data.mcp_ids as string[] | undefined) ?? [],
  };
}
export function IdentityWorkspace({
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
  const [selection, setSelection] = useState<CurrentIdentity | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [dirty, setDirty] = useState(false);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [deleteConfirm, setDeleteConfirm] = useState(false);
  useUnsavedChanges(dirty, onDirtyChange);
  const identities = records.filter(
    (e) => e.kind === "identity" && !e.heads.every((h) => h.deleted),
  );
  const current = identities.find((e) => e.id === selected);
  async function load() {
    const [all, active] = await Promise.all([
      call<{ entities: Entity[] }>("entity.list", { include_deleted: true }),
      call<CurrentIdentity>("identity.current"),
    ]);
    setRecords(all.entities);
    setSelection(active);
    setInspection(null);
  }
  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      call<{ entities: Entity[] }>("entity.list", { include_deleted: true }),
      call<CurrentIdentity>("identity.current"),
    ])
      .then(([all, active]) => {
        if (!cancelled) {
          setRecords(all.entities);
          setSelection(active);
          setInspection(null);
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [refreshSignal]);
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
  function switchTo(id: string | null, next: Draft | null = null) {
    if (dirty && !window.confirm("放弃尚未保存的身份修改？")) return;
    setSelected(id);
    setDraft(next);
    setDirty(false);
    setInspection(null);
    setDeleteConfirm(false);
    setError("");
  }
  function change<K extends keyof Draft>(key: K, value: Draft[K]) {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
    setDirty(true);
  }
  async function save() {
    if (!draft) return;
    const data = {
      ...draft.base,
      description: draft.description,
      instructions: draft.instructions,
      preferred_agent: draft.preferred_agent || null,
      capability_ids: draft.capability_ids,
      mcp_ids: draft.mcp_ids,
    };
    const result = await call<Entity>(
      draft.heads
        ? "entity.resolve"
        : draft.id
          ? "entity.update"
          : "entity.create",
      {
        ...(draft.id ? { id: draft.id } : { kind: "identity" }),
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
  function bindings(
    kind: "capability" | "mcp",
    field: "capability_ids" | "mcp_ids",
  ) {
    if (!draft) return null;
    const choices = records.filter(
      (e) =>
        e.kind === kind &&
        (!e.heads.every((h) => h.deleted) || draft[field].includes(e.id)),
    );
    const missing = draft[field].filter(
      (id) => !choices.some((e) => e.id === id),
    );
    return (
      <fieldset className="binding-field">
        <legend>
          {kind === "capability" ? "关联能力" : "关联 MCP"}{" "}
          <small>{draft[field].length}/64</small>
        </legend>
        {choices.length === 0 && missing.length === 0 && (
          <p className="help">
            暂无{kind === "capability" ? "能力" : "MCP"}
            记录。可以先保存身份，再到对应模块创建后关联。
          </p>
        )}
        {choices.map((e) => {
          const unavailable = e.conflicted || e.heads[0].deleted;
          return (
            <label className="binding-choice" key={e.id}>
              <input
                type="checkbox"
                checked={draft[field].includes(e.id)}
                disabled={unavailable && !draft[field].includes(e.id)}
                onChange={(ev) =>
                  change(
                    field,
                    ev.target.checked
                      ? [...draft[field], e.id]
                      : draft[field].filter((id) => id !== e.id),
                  )
                }
              />
              <span>
                {e.heads[0].name}
                <small>
                  {unavailable
                    ? "关联不可用，请解决冲突或取消关联"
                    : String(e.heads[0].data.description ?? "已保存记录")}
                </small>
              </span>
            </label>
          );
        })}
        {missing.map((id) => (
          <label className="binding-choice" key={id}>
            <input
              type="checkbox"
              checked
              onChange={() =>
                change(
                  field,
                  draft[field].filter((i) => i !== id),
                )
              }
            />
            <span>
              记录缺失<small>{id} · 取消关联以修复</small>
            </span>
          </label>
        ))}
      </fieldset>
    );
  }
  return (
    <>
      <div className="page-heading">
        <div>
          <div className="eyebrow">WORKSPACE / 01</div>
          <h1>身份</h1>
          <p>让角色、指引与工具关系跟随你，在每台设备上选择当前身份。</p>
        </div>
        <button
          className="primary"
          disabled={!writable || busy}
          onClick={() => switchTo(null, fromEvent())}
        >
          ＋ 新建身份
        </button>
      </div>
      {error && (
        <div className="notice error" role="alert">
          {error}
        </div>
      )}
      <div className="identity-active">
        <div>
          <span className="eyebrow">这台设备的当前身份</span>
          <h2>
            {selection?.context?.identity.name ??
              (selection?.selection.identity_id ? "身份不可用" : "尚未选择")}
          </h2>
          <p>
            {selection?.state === "unavailable"
              ? "身份或关联记录已变化，请修复后再使用；不会自动忽略失效的指引。"
              : "切换后，新生成的 agent 启动计划默认采用这个身份。其他设备各自选择。"}
          </p>
        </div>
        {selection?.selection.identity_id && (
          <button
            disabled={!writable || busy}
            onClick={() =>
              void act(async () => {
                await call("identity.clear", {
                  expected_selection_revision: selection.selection.revision,
                });
              })
            }
          >
            清除当前身份
          </button>
        )}
      </div>
      <div className="entity-workspace">
        <section className="entity-list">
          <h2>
            全部身份 <span>{identities.length}</span>
          </h2>
          <input
            className="identity-search"
            aria-label="搜索身份"
            placeholder="搜索名称或角色说明…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {identities.length === 0 && (
            <div className="empty">
              <span>◇</span>
              <h3>创建你的第一个身份</h3>
              <p>例如工作、个人研究，或某个项目中的角色。</p>
            </div>
          )}
          {identities
            .filter((e) =>
              e.heads.some((h) =>
                `${h.name} ${h.data.description ?? ""}`
                  .toLowerCase()
                  .includes(query.toLowerCase()),
              ),
            )
            .map((e) => (
              <button
                className={selected === e.id ? "entity selected" : "entity"}
                key={e.id}
                disabled={busy}
                onClick={() => switchTo(e.id)}
              >
                <strong>{e.heads[0].name}</strong>
                <small>
                  {e.conflicted
                    ? "需要合并版本"
                    : selection?.selection.identity_id === e.id
                      ? "● 本机当前身份"
                      : String(e.heads[0].data.description || "身份指引")}
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
                    ? "合并身份版本"
                    : draft.id
                      ? "编辑身份"
                      : "新建身份"}
                </h2>
                {draft.heads && (
                  <p className="notice">
                    以选中的版本为起点编辑。保存会引用全部冲突版本，保留历史。
                  </p>
                )}
                <label>
                  身份名称
                  <input
                    autoFocus
                    required
                    maxLength={200}
                    value={draft.name}
                    onChange={(e) => change("name", e.target.value)}
                  />
                </label>
                <label>
                  角色说明
                  <textarea
                    rows={2}
                    placeholder="这个身份用在什么场景？"
                    value={draft.description}
                    onChange={(e) => change("description", e.target.value)}
                  />
                </label>
                <label>
                  给 agent 的身份指引
                  <textarea
                    rows={8}
                    placeholder="描述职责、工作偏好、输出约定和应当遵循的规则…"
                    value={draft.instructions}
                    onChange={(e) => change("instructions", e.target.value)}
                  />
                </label>
                <label>
                  偏好 agent
                  <select
                    value={draft.preferred_agent}
                    onChange={(e) => change("preferred_agent", e.target.value)}
                  >
                    <option value="">每次自行选择</option>
                    {adapters.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.name}
                      </option>
                    ))}
                    {draft.preferred_agent &&
                      !adapters.some((a) => a.id === draft.preferred_agent) && (
                        <option value={draft.preferred_agent}>
                          {draft.preferred_agent}（本机未接入）
                        </option>
                      )}
                  </select>
                </label>
                {bindings("capability", "capability_ids")}
                {bindings("mcp", "mcp_ids")}
                <p className="help">
                  身份指引不提供安全隔离。账号登录与操作授权单独管理；这里只保存工具关联，不自动安装原生配置。
                </p>
                <div className="form-actions">
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => switchTo(selected)}
                  >
                    取消
                  </button>
                  <button
                    className="primary"
                    disabled={busy || !writable || !draft.name.trim()}
                  >
                    {busy ? "保存中…" : "保存身份"}
                  </button>
                </div>
              </fieldset>
            </form>
          ) : current ? (
            <>
              <div className="detail-title">
                <h2>{current.heads[0].name}</h2>
                {!current.conflicted && (
                  <button
                    disabled={!writable || busy}
                    onClick={() => {
                      setDraft(fromEvent(current.heads[0], current));
                      setDirty(false);
                    }}
                  >
                    编辑身份
                  </button>
                )}
              </div>
              {current.conflicted && (
                <div className="notice">
                  同步保留了多个并发版本。选择一个起点，编辑并合并后才能使用。
                </div>
              )}
              {current.heads.map((h, i) => (
                <article className="version" key={h.revision}>
                  <small>
                    {current.conflicted ? `版本 ${i + 1} · ` : ""}
                    {new Date(h.timestamp_ms).toLocaleString()}
                    {h.deleted ? " · 删除版本" : ""}
                  </small>
                  <h3>{h.name}</h3>
                  <p className="body-text">
                    {String(h.data.description ?? "暂无角色说明")}
                  </p>
                  <h4>身份指引</h4>
                  <p className="body-text">
                    {String(h.data.instructions || "尚未填写指引")}
                  </p>
                  <div className="identity-tags">
                    <span>
                      偏好 {String(h.data.preferred_agent || "自行选择")}
                    </span>
                    <span>
                      {((h.data.capability_ids as unknown[]) ?? []).length}{" "}
                      项能力
                    </span>
                    <span>
                      {((h.data.mcp_ids as unknown[]) ?? []).length} 个 MCP
                    </span>
                  </div>
                  {current.conflicted && (
                    <button
                      disabled={!writable || busy}
                      onClick={() => setDraft(fromEvent(h, current))}
                    >
                      以此版本编辑合并
                    </button>
                  )}
                </article>
              ))}
              {!current.conflicted && (
                <div className="form-actions">
                  <button
                    className="primary"
                    disabled={!writable || busy || !selection}
                    onClick={() =>
                      void act(async () => {
                        await call("identity.activate", {
                          id: current.id,
                          expected_revision: current.heads[0].revision,
                          expected_selection_revision:
                            selection!.selection.revision,
                        });
                      })
                    }
                  >
                    设为本机当前身份
                  </button>
                  <button
                    disabled={busy}
                    onClick={() => {
                      setError("");
                      setBusy(true);
                      void call<Inspection>("identity.inspect", {
                        id: current.id,
                      })
                        .then(setInspection)
                        .catch((e) => setError(String(e)))
                        .finally(() => setBusy(false));
                    }}
                  >
                    检查关联
                  </button>
                </div>
              )}
              {inspection && (
                <div className="identity-check">
                  <h3>
                    {inspection.ready ? "指引与关联已就绪" : "部分关联需要修复"}
                  </h3>
                  {inspection.bindings.length === 0 ? (
                    <p>这个身份暂未关联能力或 MCP。</p>
                  ) : (
                    inspection.bindings.map((b) => (
                      <p key={b.id}>
                        {b.kind === "capability" ? "能力" : "MCP"} ·{" "}
                        {b.record?.heads[0].name ?? b.id}{" "}
                        <strong>
                          {
                            (
                              {
                                ready: "就绪",
                                missing: "缺失",
                                deleted: "已删除",
                                conflicted: "存在冲突",
                                wrong_kind: "类型不符",
                              } as Record<string, string>
                            )[b.state]
                          }
                        </strong>
                      </p>
                    ))
                  )}
                  <p className="help">
                    启动计划会包含关联记录的版本快照。原生工具配置仍需独立安装。
                  </p>
                </div>
              )}
              <details>
                <summary>完整记录与版本</summary>
                <pre>{JSON.stringify(current, null, 2)}</pre>
              </details>
              {!current.conflicted && (
                <div className="identity-delete">
                  {deleteConfirm ? (
                    <>
                      <p>
                        删除会同步到其他设备，并保留历史。当前身份将显示不可用。
                      </p>
                      <button
                        disabled={busy}
                        onClick={() => setDeleteConfirm(false)}
                      >
                        取消
                      </button>{" "}
                      <button
                        className="danger"
                        disabled={busy || !writable}
                        onClick={() =>
                          void act(async () => {
                            await call("entity.delete", {
                              id: current.id,
                              expected_revision: current.heads[0].revision,
                            });
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
                      删除身份…
                    </button>
                  )}
                </div>
              )}
            </>
          ) : (
            <div className="empty">
              <span>↖</span>
              <h3>选择一个身份</h3>
              <p>查看指引、关联工具，或切换这台设备的工作身份。</p>
            </div>
          )}
        </section>
      </div>
    </>
  );
}

import { useEffect, useRef, useState } from "react";
import { Adapter, call, Entity, Event } from "./api";
import { useUnsavedChanges } from "./useUnsavedChanges";
interface Mapping {
  revision: string;
  cleared: boolean;
  executable: string;
  args: string[];
  cwd: string | null;
  env_refs: Record<string, string>;
}
interface Inspection {
  definition: Event;
  profile: Record<string, unknown>;
  mapping: Mapping | null;
  ready: boolean;
  issues: { code: string; name?: string }[];
  adaptation: { supported: boolean; error?: { message: string } };
  connection: {
    state: string;
    probe_id?: string;
    result?: unknown;
    error?: { code: string; message: string };
  };
  authorization: { probes_enabled: boolean };
}
interface Draft {
  id?: string;
  revision?: string;
  heads?: string[];
  base: Record<string, unknown>;
  name: string;
  description: string;
  server_key: string;
  transport: string;
  command_hint: string;
  endpoint: string;
  required_env: string;
  credential_refs: string;
}
function draftOf(e?: Event, r?: Entity): Draft {
  const d = e?.data ?? {};
  return {
    id: r?.id,
    revision: e?.revision,
    heads: r?.conflicted ? r.heads.map((h) => h.revision) : undefined,
    base: d,
    name: e?.name ?? "",
    description: String(d.description ?? ""),
    server_key: String(d.server_key ?? ""),
    transport: String(d.transport ?? "stdio"),
    command_hint: String(d.command_hint ?? ""),
    endpoint: String(d.endpoint ?? ""),
    required_env: ((d.required_env as string[]) ?? []).join("\n"),
    credential_refs: ((d.credential_refs as string[]) ?? []).join("\n"),
  };
}
const lines = (s: string) =>
  s
    .split("\n")
    .map((v) => v.trim())
    .filter(Boolean);
const issues: Record<string, string> = {
  server_key_missing: "尚未填写注册名称",
  mapping_missing: "这台设备尚未配置运行命令",
  executable_missing: "本机找不到可执行文件",
  executable_not_executable: "文件缺少执行权限",
  cwd_missing: "本机工作目录不存在",
  env_reference_missing: "缺少环境变量引用",
  credential_resolution_pending: "凭据引用解析尚未实现，暂不能注册或检查",
  transport_unsupported: "HTTP 定义可保存，连接与注册支持尚未实现",
};
const states: Record<string, string> = {
  not_checked: "尚未检查",
  running: "正在检查",
  succeeded: "检查成功",
  failed: "检查失败",
  timeout: "检查超时",
  cancelled: "已取消",
  stale: "定义或设备映射已变化，需重新检查",
  interrupted: "上次检查被中断",
};
export function McpWorkspace({
  writable,
  admin,
  probes,
  adapters,
  refreshSignal,
  onChange,
  onDirtyChange,
}: {
  writable: boolean;
  admin: boolean;
  probes: boolean;
  adapters: Adapter[];
  refreshSignal: unknown;
  onChange: () => Promise<void>;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [records, setRecords] = useState<Entity[]>([]),
    [selected, setSelected] = useState<string | null>(null),
    [draft, setDraft] = useState<Draft | null>(null),
    [mapDraft, setMapDraft] = useState<{
      revision: string;
      definition_revision: string;
      executable: string;
      args: string;
      cwd: string;
      env_refs: string;
    } | null>(null),
    [dirty, setDirty] = useState(false),
    [query, setQuery] = useState(""),
    [agent, setAgent] = useState("claude-code"),
    [inspection, setInspection] = useState<Inspection | null>(null),
    [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(false),
    [checking, setChecking] = useState(false),
    [error, setError] = useState(""),
    [plan, setPlan] = useState<unknown>(null),
    [deleting, setDeleting] = useState(false);
  const activeProbe = useRef<{ id: string; probe_id: string } | null>(null),
    mounted = useRef(true);
  useUnsavedChanges(dirty, onDirtyChange);
  const current = records.find((r) => r.id === selected),
    revisionKey = current?.heads.map((h) => h.revision).join(",");
  async function load() {
    setRecords(
      (await call<{ entities: Entity[] }>("entity.list", { kind: "mcp" }))
        .entities,
    );
  }
  async function inspect(id: string) {
    const i = await call<Inspection>("mcp.inspect", {
      id,
      target_agent: agent,
    });
    if (mounted.current) setInspection(i);
    return i;
  }
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (activeProbe.current)
        void call("mcp.cancel", activeProbe.current).catch(() => {});
    };
  }, []);
  useEffect(() => {
    let cancelled = false;
    void call<{ entities: Entity[] }>("entity.list", { kind: "mcp" })
      .then((v) => {
        if (!cancelled) setRecords(v.entities);
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
    if (!current || current.conflicted) {
      setInspection(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    void call<Inspection>("mcp.inspect", {
      id: current.id,
      target_agent: agent,
    })
      .then((i) => {
        if (!cancelled) setInspection(i);
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
  useEffect(() => {
    if (!checking || !selected) return;
    const timer = setInterval(
      () => void inspect(selected).catch(() => {}),
      250,
    );
    return () => clearInterval(timer);
  }, [checking, selected, agent]);
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
    if (checking) return;
    if (dirty && !window.confirm("放弃尚未保存的 MCP 修改？")) return;
    setSelected(id);
    setDraft(next);
    setMapDraft(null);
    setDirty(false);
    setError("");
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
      description: draft.description,
      server_key: draft.server_key,
      transport: draft.transport,
      command_hint: draft.command_hint,
      endpoint: draft.endpoint || null,
      required_env: lines(draft.required_env),
      credential_refs: lines(draft.credential_refs),
    };
    const r = await call<Entity>(
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
          : { kind: "mcp", name: draft.name, data },
    );
    setDraft(null);
    setDirty(false);
    setSelected(r.id);
    setPlan(null);
  }
  function editMap() {
    if (!inspection) return;
    const m = inspection.mapping;
    setMapDraft({
      revision: m?.revision ?? "none",
      definition_revision: inspection.definition.revision,
      executable: m?.executable ?? "",
      args: JSON.stringify(m?.args ?? [], null, 2),
      cwd: m?.cwd ?? "",
      env_refs: Object.entries(m?.env_refs ?? {})
        .map(([k, v]) => `${k}=${v}`)
        .join("\n"),
    });
    setDraft(null);
    setDirty(false);
  }
  async function saveMap() {
    if (!mapDraft || !selected) return;
    const env: Record<string, string> = {};
    for (const line of lines(mapDraft.env_refs)) {
      const at = line.indexOf("=");
      if (at < 1 || line.slice(0, at) in env)
        throw new Error("每行填写一个不重复的 NAME=env:VARIABLE 引用。");
      env[line.slice(0, at)] = line.slice(at + 1);
    }
    await call("mcp.map", {
      id: selected,
      expected_revision: mapDraft.definition_revision,
      expected_mapping_revision: mapDraft.revision,
      mapping: {
        executable: mapDraft.executable,
        args: JSON.parse(mapDraft.args),
        cwd: mapDraft.cwd || null,
        env_refs: env,
      },
    });
    setMapDraft(null);
    setDirty(false);
    setPlan(null);
    await inspect(selected);
  }
  async function probe() {
    if (!inspection?.mapping || !selected) return;
    const m = inspection.mapping;
    if (
      !window.confirm(
        `这将执行服务程序，程序本身可能访问文件或网络。请确认你信任它的来源。\n\n${m.executable}\n参数：${JSON.stringify(m.args)}\n目录：${m.cwd ?? "未指定"}\n\n仅做一次有超时的协议检查，不调用工具、不改原生配置。`,
      )
    )
      return;
    const job = { id: selected, probe_id: crypto.randomUUID() };
    activeProbe.current = job;
    setChecking(true);
    setError("");
    setPlan(null);
    try {
      await call("mcp.probe", {
        ...job,
        expected_revision: inspection.definition.revision,
        expected_mapping_revision: m.revision,
        confirm_execution: true,
        timeout_ms: 2000,
      });
      if (mounted.current) await inspect(selected);
    } catch (e) {
      if (mounted.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      activeProbe.current = null;
      if (mounted.current) setChecking(false);
    }
  }
  const nativeText =
    plan && typeof plan === "object" && "registration" in plan
      ? (plan as { registration?: { native_text?: string } }).registration
          ?.native_text
      : undefined;
  const available =
    !busy &&
    !loading &&
    !checking &&
    inspection?.definition.revision === current?.heads[0]?.revision;
  return (
    <>
      <div className="page-heading">
        <div>
          <div className="eyebrow">WORKSPACE / 04</div>
          <h1>MCP</h1>
          <p>同步连接定义，在各台设备配置运行环境，检查后准备 agent 注册。</p>
        </div>
        <button
          className="primary"
          disabled={!writable || busy || checking}
          onClick={() => choose(null, draftOf())}
        >
          ＋ 新建 MCP
        </button>
      </div>
      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      <div className="entity-workspace">
        <section className="entity-list">
          <h2>
            连接定义<span>{records.length}</span>
          </h2>
          <input
            aria-label="搜索 MCP"
            className="identity-search"
            placeholder="搜索名称"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {records
            .filter((r) =>
              r.heads.some((h) =>
                h.name.toLowerCase().includes(query.toLowerCase()),
              ),
            )
            .map((r) => (
              <button
                key={r.id}
                className={selected === r.id ? "entity selected" : "entity"}
                disabled={busy || checking}
                onClick={() => choose(r.id)}
              >
                <strong>{r.heads[0].name}</strong>
                <small>
                  {r.conflicted
                    ? "需要解决冲突"
                    : String(r.heads[0].data.transport ?? "stdio")}
                </small>
              </button>
            ))}
          {records.length === 0 && (
            <div className="empty">
              <span>◇</span>
              <h3>连接你的工具</h3>
              <p>先创建定义，再配置这台设备的命令。不会自动运行服务。</p>
            </div>
          )}
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
                    ? "整理 MCP 冲突"
                    : draft.id
                      ? "编辑 MCP 定义"
                      : "新建 MCP 定义"}
                </h2>
                <label>
                  MCP 名称
                  <input
                    required
                    value={draft.name}
                    onChange={(e) => change("name", e.target.value)}
                    maxLength={200}
                  />
                </label>
                <label>
                  注册名称
                  <input
                    required
                    placeholder="例如 filesystem，避免与已有配置重名"
                    value={draft.server_key}
                    onChange={(e) => change("server_key", e.target.value)}
                    maxLength={128}
                  />
                </label>
                <label>
                  说明
                  <textarea
                    rows={2}
                    value={draft.description}
                    onChange={(e) => change("description", e.target.value)}
                  />
                </label>
                <label>
                  连接方式
                  <select
                    value={draft.transport}
                    onChange={(e) => change("transport", e.target.value)}
                  >
                    <option value="stdio">stdio · 本机程序</option>
                    <option value="http">
                      HTTP · 先保存定义，连接支持待实现
                    </option>
                  </select>
                </label>
                {draft.transport === "stdio" ? (
                  <label>
                    程序提示
                    <input
                      placeholder="例如 python3 或 uvx；不要填写本机路径"
                      value={draft.command_hint}
                      onChange={(e) => change("command_hint", e.target.value)}
                    />
                  </label>
                ) : (
                  <label>
                    HTTPS MCP 地址
                    <input
                      placeholder="https://…/mcp，不含凭据或查询参数"
                      value={draft.endpoint}
                      onChange={(e) => change("endpoint", e.target.value)}
                    />
                  </label>
                )}
                <label>
                  需要的环境变量名称
                  <textarea
                    rows={2}
                    placeholder="每行一个名称"
                    value={draft.required_env}
                    onChange={(e) => change("required_env", e.target.value)}
                  />
                </label>
                <label>
                  凭据引用
                  <textarea
                    rows={2}
                    placeholder="例如 keychain:work-mcp 或 env:MCP_CREDENTIAL"
                    value={draft.credential_refs}
                    onChange={(e) => change("credential_refs", e.target.value)}
                  />
                </label>
                <p className="help">
                  这里只同步定义与引用。敏感值、本机路径和运行结果不会放入定义；凭据解析与
                  HTTP 连接尚未实现。
                </p>
                {draft.heads && (
                  <p className="notice">
                    保存将引用全部冲突
                    heads；旧版本与删除历史保留。请检查下面其他版本的字段。
                  </p>
                )}
                <div className="capability-actions">
                  <button className="primary">保存 MCP 定义</button>
                  <button type="button" onClick={() => choose(selected)}>
                    取消编辑
                  </button>
                </div>
              </fieldset>
            </form>
          ) : mapDraft ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void act(saveMap);
              }}
            >
              <fieldset
                className="identity-editor"
                disabled={busy || !admin || !writable}
              >
                <h2>这台设备的运行环境</h2>
                <label>
                  可执行文件
                  <input
                    required
                    placeholder="绝对路径"
                    value={mapDraft.executable}
                    onChange={(e) => {
                      setMapDraft({ ...mapDraft, executable: e.target.value });
                      setDirty(true);
                    }}
                  />
                </label>
                <label>
                  命令参数（JSON 数组）
                  <textarea
                    rows={5}
                    value={mapDraft.args}
                    onChange={(e) => {
                      setMapDraft({ ...mapDraft, args: e.target.value });
                      setDirty(true);
                    }}
                  />
                </label>
                <label>
                  运行目录（可选）
                  <input
                    value={mapDraft.cwd}
                    onChange={(e) => {
                      setMapDraft({ ...mapDraft, cwd: e.target.value });
                      setDirty(true);
                    }}
                  />
                </label>
                <label>
                  本机环境引用
                  <textarea
                    rows={3}
                    placeholder="NAME=env:VARIABLE；不要填敏感值"
                    value={mapDraft.env_refs}
                    onChange={(e) => {
                      setMapDraft({ ...mapDraft, env_refs: e.target.value });
                      setDirty(true);
                    }}
                  />
                </label>
                <p className="help">
                  映射仅留在这台设备。参数不要填敏感值；保存不会执行程序。当前凭据引用解析尚未支持，填入引用会阻止注册与检查。三个注册格式暂不支持安全表示
                  cwd，可留空或使用自己审查过的启动器。
                </p>
                <div className="capability-actions">
                  <button className="primary">保存本机映射</button>
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
                    : String(current.heads[0].data.transport ?? "stdio")}
                </span>
              </div>
              {current.conflicted ? (
                <>
                  <p className="notice">先整理定义，设备映射会保留。</p>
                  {current.heads.map((h) => (
                    <article className="version" key={h.revision}>
                      <small>
                        {h.deleted ? "删除版本" : "保留版本"} · {h.revision}
                      </small>
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
              ) : (
                <>
                  <p className="body-text">
                    {String(current.heads[0].data.description ?? "")}
                  </p>
                  <button
                    disabled={!writable || busy || checking}
                    onClick={() =>
                      choose(current.id, draftOf(current.heads[0], current))
                    }
                  >
                    编辑 MCP 定义
                  </button>
                  <label>
                    目标 agent
                    <select
                      value={agent}
                      disabled={busy || checking}
                      onChange={(e) => setAgent(e.target.value)}
                    >
                      {adapters.map((a) => (
                        <option key={a.id} value={a.id}>
                          {a.name}
                        </option>
                      ))}
                    </select>
                  </label>
                  {loading && <p className="help">正在核对定义与本机状态…</p>}
                  {inspection && (
                    <>
                      {inspection.adaptation.error && (
                        <p className="notice">
                          适配限制：{inspection.adaptation.error.message}
                        </p>
                      )}
                      <div className="mcp-status-grid">
                        <div>
                          <small>定义</small>
                          <strong>
                            {inspection.profile.server_key
                              ? "有效版本"
                              : "待完善"}
                          </strong>
                        </div>
                        <div>
                          <small>适配</small>
                          <strong>
                            {inspection.adaptation.supported
                              ? "可生成注册文档"
                              : "尚未支持"}
                          </strong>
                        </div>
                        <div>
                          <small>连接</small>
                          <strong>
                            {checking
                              ? "正在检查"
                              : (states[inspection.connection.state] ??
                                inspection.connection.state)}
                          </strong>
                        </div>
                        <div>
                          <small>授权</small>
                          <strong>
                            {probes ? "本次确认后检查" : "程序检查未启用"}
                          </strong>
                        </div>
                      </div>
                      <div className="identity-check">
                        <h3>本机环境与检查</h3>
                        <ul>
                          {inspection.issues.map((i, n) => (
                            <li key={n}>
                              {issues[i.code] ?? i.code}
                              {i.name ? ` · ${i.name}` : ""}
                            </li>
                          ))}
                        </ul>
                        {inspection.mapping && !inspection.mapping.cleared && (
                          <>
                            <p>
                              <code>{inspection.mapping.executable}</code>
                            </p>
                            <pre>
                              {JSON.stringify(inspection.mapping.args, null, 2)}
                            </pre>
                            <p className="help">
                              运行目录：{inspection.mapping.cwd ?? "未指定"}
                            </p>
                          </>
                        )}
                        <div className="capability-actions">
                          <button
                            disabled={!admin || !writable || !available}
                            onClick={editMap}
                          >
                            {inspection.mapping && !inspection.mapping.cleared
                              ? "编辑本机映射"
                              : "配置本机映射"}
                          </button>
                          <button
                            disabled={!available}
                            onClick={() =>
                              void act(async () => {
                                await inspect(current.id);
                              })
                            }
                          >
                            重新核对环境
                          </button>
                        </div>
                        {checking ? (
                          <button
                            className="danger"
                            onClick={() => {
                              const job = activeProbe.current;
                              if (job)
                                void call("mcp.cancel", job).catch((e) =>
                                  setError(String(e)),
                                );
                            }}
                          >
                            取消连接检查
                          </button>
                        ) : (
                          <button
                            className="primary"
                            disabled={
                              !available ||
                              !inspection.ready ||
                              !writable ||
                              !admin ||
                              !probes
                            }
                            onClick={() => void probe()}
                          >
                            检查连接（需确认）
                          </button>
                        )}
                        {!probes && (
                          <p className="help">
                            此 Web/MCP
                            入口未允许执行程序。需要检查时，由你主动以
                            --allow-writes --allow-admin --allow-mcp-probes
                            启动本机服务；现有权限不会自动扩大。
                          </p>
                        )}
                        <p className="help">
                          连接成功只表示这一版命令通过初始化与目录读取；不等于服务可信、工具已授权或已装入
                          agent。服务自己的执行行为不受这次协议检查隔离。
                        </p>
                        {inspection.connection.error && (
                          <p role="status">
                            检查结果：{inspection.connection.error.message}
                          </p>
                        )}
                        {inspection.connection.result != null && (
                          <details>
                            <summary>协议检查结果</summary>
                            <pre>
                              {JSON.stringify(
                                inspection.connection.result,
                                null,
                                2,
                              )}
                            </pre>
                          </details>
                        )}
                      </div>
                      <button
                        disabled={
                          !available ||
                          !inspection.ready ||
                          !inspection.adaptation.supported
                        }
                        onClick={() =>
                          void act(async () => {
                            setPlan(
                              await call("mcp.prepare", {
                                id: current.id,
                                expected_revision:
                                  inspection.definition.revision,
                                target_agent: agent,
                              }),
                            );
                          })
                        }
                      >
                        生成 MCP 注册计划
                      </button>
                      <p className="help">
                        只生成选定 agent
                        的文档数据，不写个人配置。请确认注册名称没有覆盖已有连接。运行目录或其他格式限制会明确报错。
                      </p>
                      {plan != null && (
                        <details open>
                          <summary>MCP 注册计划</summary>
                          {nativeText && (
                            <label>
                              可复制的注册文档
                              <textarea readOnly rows={9} value={nativeText} />
                            </label>
                          )}
                          <pre>{JSON.stringify(plan, null, 2)}</pre>
                        </details>
                      )}
                      {inspection.mapping && !inspection.mapping.cleared && (
                        <button
                          disabled={!admin || !writable || !available}
                          onClick={() => {
                            if (
                              window.confirm(
                                "清除这台设备的映射？同步定义会保留，后续需要重新配置。",
                              )
                            )
                              void act(async () => {
                                await call("mcp.clear_mapping", {
                                  id: current.id,
                                  expected_mapping_revision:
                                    inspection.mapping!.revision,
                                });
                                setPlan(null);
                                await inspect(current.id);
                              });
                          }}
                        >
                          清除本机映射
                        </button>
                      )}
                    </>
                  )}
                  <div className="identity-delete">
                    {deleting ? (
                      <>
                        <p>
                          删除只产生可同步的删除版本；关联身份会变为不可用，本机映射保留。
                        </p>
                        <button
                          className="danger"
                          disabled={!writable || busy || checking}
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
                          确认删除 MCP
                        </button>
                        <button onClick={() => setDeleting(false)}>取消</button>
                      </>
                    ) : (
                      <button
                        className="danger"
                        disabled={!writable || busy || checking}
                        onClick={() => setDeleting(true)}
                      >
                        删除 MCP 定义
                      </button>
                    )}
                  </div>
                </>
              )}
            </>
          ) : (
            <div className="empty">
              <span>◇</span>
              <h3>选择一个 MCP 定义</h3>
              <p>连接状态与授权状态分别核对，避免把保存记录当作已连接。</p>
            </div>
          )}
        </section>
      </div>
    </>
  );
}

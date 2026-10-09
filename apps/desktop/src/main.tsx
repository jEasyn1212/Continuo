import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  Adapter,
  call,
  connect,
  Connection,
  desktopAvailable,
  Entity,
  Kind,
  Status,
} from "./api";
import "./style.css";
import { TaskWorkspace } from "./TaskWorkspace";
import { IdentityWorkspace, CurrentIdentity } from "./IdentityWorkspace";

const domains: {
  kind: Kind;
  label: string;
  mark: string;
  description: string;
}[] = [
  {
    kind: "identity",
    label: "身份",
    mark: "01",
    description: "在不同角色中保持明确的偏好、规则与工作上下文。",
  },
  {
    kind: "task",
    label: "任务",
    mark: "02",
    description: "记录目标、决策与下一步，让工作可以继续。",
  },
  {
    kind: "capability",
    label: "能力",
    mark: "03",
    description: "管理技能、规则与版本，按需要连接到 agent。",
  },
  {
    kind: "mcp",
    label: "MCP",
    mark: "04",
    description: "记录工具连接和凭据引用，分别检查设备环境。",
  },
  {
    kind: "session",
    label: "会话",
    mark: "05",
    description: "将原生会话与任务、身份及设备关联。",
  },
  {
    kind: "device",
    label: "设备",
    mark: "06",
    description: "记录设备与环境，映射各自的执行位置。",
  },
];
type Page = "overview" | Kind | "agents" | "sync" | "api";

function App() {
  const [connection, setConnection] = useState<Connection | null>(null);
  const [hasUnsaved, setHasUnsaved] = useState(false);
  const [page, setPage] = useState<Page>("overview");
  const [status, setStatus] = useState<Status | null>(null);
  const [entities, setEntities] = useState<Entity[]>([]);
  const [adapters, setAdapters] = useState<Adapter[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [agentIdentity, setAgentIdentity] = useState("current");
  const [activeIdentity, setActiveIdentity] = useState<CurrentIdentity | null>(
    null,
  );
  const [agent, setAgent] = useState("claude-code");
  const [cwd, setCwd] = useState("");
  const [prompt, setPrompt] = useState("");
  const [plan, setPlan] = useState<unknown>(null);
  const [remote, setRemote] = useState("");
  const [keyFile, setKeyFile] = useState("");
  const [syncResult, setSyncResult] = useState<unknown>(null);
  const [apiMethod, setApiMethod] = useState("system.status");
  const [apiInput, setApiInput] = useState("{}");
  const [apiResult, setApiResult] = useState<unknown>(null);
  const selectedTool = connection?.tools?.find(
    (tool) => tool.name === `continuo_${apiMethod.replaceAll(".", "_")}`,
  );
  const domain = domains.find((d) => d.kind === page);
  const current = entities.find((e) => e.id === selected);

  function navigate(next: Page) {
    if (next === page) return;
    if (hasUnsaved && !window.confirm("放弃尚未保存的工作记录并切换模块？"))
      return;
    setHasUnsaved(false);
    setPage(next);
  }
  async function refresh() {
    try {
      const nextConnection = await connect();
      const [nextStatus, nextEntities, nextAdapters, nextIdentity] =
        await Promise.all([
          call<Status>("system.status"),
          call<{ entities: Entity[] }>(
            "entity.list",
            domain ? { kind: domain.kind } : {},
          ),
          call<{ adapters: Adapter[] }>("agent.list"),
          call<CurrentIdentity>("identity.current"),
        ]);
      setStatus(nextStatus);
      setEntities(nextEntities.entities);
      setAdapters(nextAdapters.adapters);
      setActiveIdentity(nextIdentity);
      setConnection(nextConnection);
    } catch (error) {
      setConnection(null);
      setStatus(null);
      setEntities([]);
      setAdapters([]);
      throw error;
    }
  }
  const connected = connection !== null;
  const writable = connected && connection.permissions.writes;
  async function action(fn: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await fn();
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  useEffect(() => {
    setSelected(null);
    setEditing(false);
    setError("");
    void refresh().catch((e) => setError(String(e)));
  }, [page]);

  useEffect(() => {
    if (page !== "agents" || agentIdentity !== "current") return;
    const preferred = activeIdentity?.context?.identity.data.preferred_agent;
    if (
      typeof preferred === "string" &&
      adapters.some((a) => a.id === preferred)
    )
      setAgent(preferred);
  }, [page, agentIdentity, activeIdentity?.selection.revision]);

  function newEntity() {
    setName("");
    setDescription("");
    setEditing(true);
    setSelected(null);
  }
  function editEntity() {
    if (!current || current.conflicted) return;
    setName(current.heads[0].name);
    setDescription(
      String(
        current.heads[0].data.description ??
          current.heads[0].data.instructions ??
          "",
      ),
    );
    setEditing(true);
  }
  async function save() {
    if (!domain || !name.trim()) return;
    const field = domain.kind === "identity" ? "instructions" : "description";
    if (current) {
      const head = current.heads[0];
      await call("entity.update", {
        id: current.id,
        expected_revision: head.revision,
        name,
        data: { ...head.data, [field]: description },
      });
    } else {
      const created = await call<Entity>("entity.create", {
        kind: domain.kind,
        name,
        data: { [field]: description },
      });
      setSelected(created.id);
    }
    setEditing(false);
  }

  return (
    <div className="shell">
      <aside className="sidebar">
        <a
          className="brand"
          href="#"
          onClick={(e) => {
            e.preventDefault();
            navigate("overview");
          }}
          aria-label="Continuo 概览"
        >
          <svg width="30" height="30" viewBox="0 0 30 30" aria-hidden="true">
            <path
              d="M23 8a10 10 0 1 0 0 14M7 8a10 10 0 0 1 0 14"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.5"
              strokeLinecap="round"
            />
          </svg>
          <span>
            Continuo<small>工作，持续接续。</small>
          </span>
        </a>
        <div className="workspace">
          <span className="dot" />
          <div>
            本地工作空间
            <small>
              {connected
                ? connection.mode === "app"
                  ? "App · 已连接本机核心"
                  : "Web · 已连接本机核心"
                : "等待本机连接"}
            </small>
          </div>
        </div>
        <nav aria-label="主导航">
          <button
            className={page === "overview" ? "active" : ""}
            onClick={() => navigate("overview")}
          >
            <span>◈</span>概览
          </button>
          <p className="nav-label">工作环境</p>
          {domains.map((d) => (
            <button
              key={d.kind}
              className={page === d.kind ? "active" : ""}
              onClick={() => navigate(d.kind)}
            >
              <span className="nav-number">{d.mark}</span>
              {d.label}
              <i>{status?.counts[d.kind] ?? "—"}</i>
            </button>
          ))}
          <p className="nav-label">连接</p>
          <button
            className={page === "agents" ? "active" : ""}
            onClick={() => navigate("agents")}
          >
            <span>↗</span>Agent 适配
          </button>
          <button
            className={page === "sync" ? "active" : ""}
            onClick={() => navigate("sync")}
          >
            <span>⇄</span>跨设备同步
          </button>
          <button
            className={page === "api" ? "active" : ""}
            onClick={() => navigate("api")}
          >
            <span>⌘</span>接口控制台
          </button>
        </nav>
        <div className="sidebar-foot">
          <span className="pill">本地优先</span>
          <small>0.1.1 · App / Web</small>
        </div>
      </aside>
      <main>
        <header className="topbar">
          <span>
            个人 agent 工作环境 <b>/</b>{" "}
            {domain?.label ??
              (
                {
                  overview: "概览",
                  agents: "Agent 适配",
                  sync: "跨设备同步",
                  api: "接口控制台",
                } as Record<string, string>
              )[page]}
          </span>
          <button disabled={busy} onClick={() => void action(refresh)}>
            刷新状态 ↻
          </button>
        </header>
        <div className="content">
          {!desktopAvailable && !connected && (
            <div className="notice" role="status">
              <strong>连接本机 Continuo Web</strong>
              <p>
                在项目根目录构建 CLI，在 apps/desktop
                中构建页面并启动本机服务。然后打开终端显示的
                http://127.0.0.1:1421。
              </p>
              <pre>
                {
                  "cargo build -p continuo-cli\ncd apps/desktop\nnpm ci && npm run build\nnpm run web -- --allow-writes --allow-admin"
                }
              </pre>
              <p>
                普通浏览器通过本机服务读写数据；App 使用原生接口。直接打开 HTML
                文件或仅运行 Vite 不会连接数据。
              </p>
              <button onClick={() => void action(refresh)} disabled={busy}>
                重新连接 ↻
              </button>
            </div>
          )}
          {connection?.mode === "web" && (
            <div className="notice" role="status">
              Web 已连接当前设备 ·{" "}
              {connection.permissions.writes ? "可写入" : "只读"} ·{" "}
              {connection.permissions.admin ? "可管理配置" : "管理未授权"} ·{" "}
              {connection.permissions.sync ? "同步已授权" : "同步未授权"}
              {!connection.permissions.writes && (
                <p>
                  需要编辑时，以 --allow-writes 重启本机服务；管理配置另加
                  --allow-admin，同步另加 --allow-sync。
                </p>
              )}
            </div>
          )}
          {error && (
            <div className="error" role="alert">
              {error}
            </div>
          )}
          {page === "overview" && (
            <>
              <div className="hero">
                <div className="eyebrow">CONTINUITY, BY DESIGN</div>
                <h1>
                  让每一次开始，
                  <br />
                  <em>接得上上一次。</em>
                </h1>
                <p>
                  身份、任务、能力与工具连接，在不同 agent 和设备之间保持清晰。
                </p>
                <button
                  className="primary"
                  disabled={!connected}
                  onClick={() => {
                    navigate("identity");
                  }}
                >
                  管理我的工作环境 <span>↗</span>
                </button>
                <div className="hero-orbit" aria-hidden="true">
                  <div />
                  <div />
                  <div />
                  <span>C</span>
                </div>
              </div>
              <div className="section-title">
                <h2>你的工作环境</h2>
                <span>六个领域，一个工作空间</span>
              </div>
              <div className="domain-grid">
                {domains.map((d) => (
                  <button
                    className="domain-card"
                    key={d.kind}
                    onClick={() =>
                      navigate(d.kind === "device" ? "sync" : d.kind)
                    }
                  >
                    <div>
                      <span className="card-mark">{d.mark}</span>
                      <span className="arrow">↗</span>
                    </div>
                    <h3>
                      {d.kind === "device" ? "跨设备同步" : d.label}
                      <strong>
                        {d.kind === "device"
                          ? "⇄"
                          : (status?.counts[d.kind] ?? "—")}
                      </strong>
                    </h3>
                    <p>
                      {d.kind === "device"
                        ? "使用你自己的存储，在设备之间携带工作环境和进展。"
                        : d.description}
                    </p>
                  </button>
                ))}
              </div>
              <div className="status-row">
                <div>
                  <span className="dot" />
                  <strong>设备本地保存</strong>
                  <p>
                    {status
                      ? `${status.event_count} 条版本记录`
                      : "等待本机连接"}
                  </p>
                </div>
                <div>
                  <strong>
                    {status?.sync_configured
                      ? "已配置同步存储"
                      : "同步存储待配置"}
                  </strong>
                  <p>使用你自己的 GitHub 私有仓库</p>
                </div>
                <div>
                  <strong>
                    {status
                      ? `${status.conflicts} 个待解决冲突`
                      : "冲突状态未知"}
                  </strong>
                  <p>并发修改保留双方版本</p>
                </div>
              </div>
            </>
          )}
          {page === "identity" && connected && (
            <IdentityWorkspace
              writable={writable}
              adapters={adapters}
              refreshSignal={status}
              onChange={refresh}
              onDirtyChange={setHasUnsaved}
            />
          )}
          {page === "task" && connected && (
            <TaskWorkspace
              writable={writable}
              adapters={adapters}
              refreshSignal={status}
              onChange={refresh}
              onDirtyChange={setHasUnsaved}
            />
          )}
          {domain && page !== "identity" && page !== "task" && (
            <>
              <div className="page-heading">
                <div>
                  <div className="eyebrow">WORKSPACE / {domain.mark}</div>
                  <h1>{domain.label}</h1>
                  <p>{domain.description}</p>
                </div>
                <button
                  className="primary"
                  disabled={!writable || busy}
                  onClick={newEntity}
                >
                  ＋ 新建{domain.label}
                </button>
              </div>
              <div className="entity-workspace">
                <section className="entity-list">
                  <h2>
                    全部{domain.label}
                    <span>{entities.length}</span>
                  </h2>
                  {entities.length === 0 && (
                    <div className="empty">
                      <span>◇</span>
                      <h3>从第一条{domain.label}开始</h3>
                      <p>创建后保存在本机，配置同步后可以带到其他设备。</p>
                    </div>
                  )}
                  {entities.map((e) => (
                    <button
                      className={
                        selected === e.id ? "entity selected" : "entity"
                      }
                      key={e.id}
                      onClick={() => {
                        setSelected(e.id);
                        setEditing(false);
                      }}
                    >
                      <strong>{e.heads[0].name}</strong>
                      <small>
                        {e.conflicted ? "需要解决冲突" : "已保存到本机"}
                      </small>
                    </button>
                  ))}
                </section>
                <section className="detail">
                  {editing ? (
                    <form
                      onSubmit={(e) => {
                        e.preventDefault();
                        void action(save);
                      }}
                    >
                      <h2>
                        {current ? "编辑" : "新建"}
                        {domain.label}
                      </h2>
                      <label>
                        名称
                        <input
                          autoFocus
                          required
                          value={name}
                          onChange={(e) => setName(e.target.value)}
                          maxLength={200}
                        />
                      </label>
                      <label>
                        {domain.kind === "identity"
                          ? "身份指引"
                          : "说明与工作记录"}
                        <textarea
                          value={description}
                          onChange={(e) => setDescription(e.target.value)}
                          rows={9}
                        />
                      </label>
                      <p className="help">
                        记录明确的上下文和凭据引用。登录凭据单独保存在设备上。
                      </p>
                      <div className="form-actions">
                        <button type="button" onClick={() => setEditing(false)}>
                          取消
                        </button>
                        <button
                          className="primary"
                          disabled={busy || !writable || !name.trim()}
                        >
                          保存到本机
                        </button>
                      </div>
                    </form>
                  ) : current ? (
                    <>
                      <div className="detail-title">
                        <h2>{current.heads[0].name}</h2>
                        {!current.conflicted && (
                          <button onClick={editEntity} disabled={!writable}>
                            编辑
                          </button>
                        )}
                      </div>
                      {current.conflicted && (
                        <div className="notice">
                          不同设备产生了多个版本。选择保留的版本，或通过 agent
                          接口提交合并后的内容。
                        </div>
                      )}
                      {current.heads.map((head, index) => (
                        <article className="version" key={head.revision}>
                          <small>
                            {current.conflicted ? `版本 ${index + 1} · ` : ""}
                            {new Date(head.timestamp_ms).toLocaleString()}{" "}
                            {head.deleted ? "· 已删除" : ""}
                          </small>
                          <p className="body-text">
                            {String(
                              head.data.instructions ??
                                head.data.description ??
                                "暂无说明",
                            )}
                          </p>
                          {current.conflicted && (
                            <button
                              disabled={busy || !writable}
                              onClick={() =>
                                void action(async () => {
                                  await call("entity.resolve", {
                                    id: current.id,
                                    expected_heads: current.heads.map(
                                      (h) => h.revision,
                                    ),
                                    name: head.name,
                                    data: head.data,
                                    deleted: head.deleted,
                                  });
                                })
                              }
                            >
                              保留这个版本
                            </button>
                          )}
                        </article>
                      ))}
                      <details>
                        <summary>查看完整记录</summary>
                        <pre>{JSON.stringify(current, null, 2)}</pre>
                      </details>
                    </>
                  ) : (
                    <div className="empty">
                      <span>↖</span>
                      <h3>选择一条{domain.label}</h3>
                      <p>在这里查看内容与版本。</p>
                    </div>
                  )}
                </section>
              </div>
            </>
          )}
          {page === "agents" && (
            <>
              <div className="page-heading">
                <div>
                  <div className="eyebrow">AGENT ADAPTERS</div>
                  <h1>不同 agent，同一套工作入口。</h1>
                  <p>
                    首版接入 Claude
                    Code、Codex、Hermes。接口能力由适配器分别声明。
                  </p>
                </div>
              </div>
              <div className="adapter-grid">
                {adapters.map((a) => (
                  <article className="adapter-card" key={a.id}>
                    <span className="pill">启动计划</span>
                    <h2>{a.name}</h2>
                    <p>原生恢复参数 · MCP 注册文档</p>
                    <small>尚未验证本机登录与运行时版本</small>
                  </article>
                ))}
              </div>
              <form
                className="settings-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  void action(async () =>
                    setPlan(
                      await call("agent.prepare", {
                        agent,
                        cwd,
                        ...(prompt ? { prompt } : {}),
                        ...(agentIdentity === "none"
                          ? { use_current_identity: false }
                          : agentIdentity !== "current"
                            ? { identity_id: agentIdentity }
                            : {}),
                      }),
                    ),
                  );
                }}
              >
                <h2>准备启动计划</h2>
                <label>
                  Agent
                  <select
                    value={agent}
                    onChange={(e) => setAgent(e.target.value)}
                  >
                    <option value="claude-code">Claude Code</option>
                    <option value="codex">Codex</option>
                    <option value="hermes">Hermes</option>
                  </select>
                </label>
                <label>
                  工作身份
                  <select
                    value={agentIdentity}
                    onChange={(e) => {
                      setAgentIdentity(e.target.value);
                      setPlan(null);
                      const profile =
                        e.target.value === "current"
                          ? activeIdentity?.context?.identity.data
                          : entities.find((item) => item.id === e.target.value)
                              ?.heads[0].data;
                      if (
                        typeof profile?.preferred_agent === "string" &&
                        adapters.some((a) => a.id === profile.preferred_agent)
                      )
                        setAgent(profile.preferred_agent);
                    }}
                  >
                    <option value="current">
                      使用本机当前身份 ·{" "}
                      {activeIdentity?.context?.identity.name ??
                        (activeIdentity?.state === "unavailable"
                          ? "不可用"
                          : "未选择")}
                    </option>
                    <option value="none">本次不使用身份</option>
                    {entities
                      .filter(
                        (e) =>
                          e.kind === "identity" &&
                          !e.conflicted &&
                          !e.heads[0].deleted,
                      )
                      .map((e) => (
                        <option key={e.id} value={e.id}>
                          {e.heads[0].name}
                        </option>
                      ))}
                  </select>
                </label>
                <label>
                  本机工作目录
                  <input
                    required
                    placeholder="绝对路径"
                    value={cwd}
                    onChange={(e) => setCwd(e.target.value)}
                  />
                </label>
                <label>
                  开始时的工作指令
                  <textarea
                    value={prompt}
                    onChange={(e) => setPrompt(e.target.value)}
                    rows={3}
                  />
                </label>
                <button className="primary" disabled={busy || !connected}>
                  生成计划
                </button>
                <p className="help">
                  当前只生成计划，不启动进程或改写原生配置。
                </p>
                {plan != null && <pre>{JSON.stringify(plan, null, 2)}</pre>}
              </form>
            </>
          )}
          {page === "api" && (
            <>
              <div className="page-heading">
                <div>
                  <div className="eyebrow">SHARED OPERATIONS</div>
                  <h1>每个入口，同一套接口。</h1>
                  <p>
                    这里列出当前入口获授权的操作，与 agent 的 MCP
                    工具共享业务核心。输入真实参数，结果保存在当前设备。
                  </p>
                </div>
              </div>
              <form
                className="settings-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  void action(async () => {
                    const params: unknown = JSON.parse(apiInput);
                    if (
                      !params ||
                      typeof params !== "object" ||
                      Array.isArray(params)
                    )
                      throw new Error("参数必须是 JSON 对象。");
                    setApiResult(
                      await call(apiMethod, params as Record<string, unknown>),
                    );
                  });
                }}
              >
                <h2>调用接口</h2>
                <label>
                  操作
                  <select
                    value={apiMethod}
                    onChange={(e) => {
                      setApiMethod(e.target.value);
                      setApiInput("{}");
                      setApiResult(null);
                    }}
                  >
                    {connection?.tools?.map((tool) => {
                      const method = tool.name.replace(
                        /^continuo_([a-z]+)_/,
                        "$1.",
                      );
                      return (
                        <option key={tool.name} value={method}>
                          {method}
                        </option>
                      );
                    })}
                  </select>
                </label>
                <p className="help">{selectedTool?.description}</p>
                <label>
                  JSON 参数
                  <textarea
                    rows={8}
                    value={apiInput}
                    onChange={(e) => setApiInput(e.target.value)}
                    spellCheck={false}
                  />
                </label>
                <button
                  className="primary"
                  disabled={busy || !connected || !selectedTool}
                >
                  调用 {apiMethod}
                </button>
                {selectedTool && (
                  <details>
                    <summary>查看参数格式</summary>
                    <pre>
                      {JSON.stringify(selectedTool.inputSchema, null, 2)}
                    </pre>
                  </details>
                )}
                {apiResult !== null && (
                  <pre aria-label="接口结果">
                    {JSON.stringify(apiResult, null, 2)}
                  </pre>
                )}
              </form>
            </>
          )}
          {page === "sync" && (
            <>
              <div className="page-heading">
                <div>
                  <div className="eyebrow">YOUR STORAGE, YOUR DATA</div>
                  <h1>把工作带到另一台设备。</h1>
                  <p>使用你自己的存储。Continuo 首版无需部署统一服务端。</p>
                </div>
              </div>
              <div className="sync-diagram">
                <span>
                  本机
                  <br />
                  <small>本地数据</small>
                </span>
                <b>⇄</b>
                <span>
                  你的 GitHub 私有仓库
                  <br />
                  <small>加密版本记录</small>
                </span>
                <b>⇄</b>
                <span>
                  另一台设备
                  <br />
                  <small>本地数据</small>
                </span>
              </div>
              <form
                className="settings-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  void action(async () => {
                    await call("sync.configure", { remote, key_file: keyFile });
                    setSyncResult({ configured: true });
                  });
                }}
              >
                <h2>配置同步存储</h2>
                <label>
                  专用私有仓库地址
                  <input
                    required
                    placeholder="https://github.com/you/continuo-data.git"
                    value={remote}
                    onChange={(e) => setRemote(e.target.value)}
                  />
                </label>
                <label>
                  本机加密密钥文件
                  <input
                    required
                    placeholder="已有密钥文件的绝对路径"
                    value={keyFile}
                    onChange={(e) => setKeyFile(e.target.value)}
                  />
                </label>
                <p className="help">
                  开发预览使用本地密钥文件。两台设备需要同一把密钥；Git
                  身份验证使用本机已有的 Git 认证。密钥不上传到仓库。
                </p>
                <div className="form-actions">
                  <button disabled={busy || !connection?.permissions.admin}>
                    保存配置
                  </button>
                  <button
                    className="primary"
                    type="button"
                    disabled={
                      busy ||
                      !connection?.permissions.sync ||
                      !status?.sync_configured
                    }
                    onClick={() =>
                      void action(async () =>
                        setSyncResult(await call("sync.run")),
                      )
                    }
                  >
                    {busy ? "正在处理…" : "立即同步 ⇄"}
                  </button>
                </div>
                {syncResult != null && (
                  <pre>{JSON.stringify(syncResult, null, 2)}</pre>
                )}
              </form>
            </>
          )}
        </div>
        <footer>
          Continuo 0.3.0{" "}
          <span>本地优先 · 用户自选存储 · App / Web / CLI / MCP 共享核心</span>
        </footer>
      </main>
    </div>
  );
}
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

import { useEffect, useRef, useState } from "react";
import { Adapter, call, desktopAvailable, Entity } from "./api";
import { useUnsavedChanges } from "./useUnsavedChanges";
interface Selection {
  expected_revision: string;
  active: {
    revision: string;
    parent: string | null;
    relative_path: string;
  } | null;
  integrity: {
    ready: boolean;
    home?: string;
    error?: { message: string };
  } | null;
}
interface Plan {
  plan_digest: string;
  file: { relative_path: string; content: string };
}
interface Run {
  run_id: string;
  target_agent: string;
  deployment_revision: string;
  control_revision: string;
  state: string;
  observed_state: string;
  owner_present: boolean;
  termination_verified: boolean;
  stdout: string;
  stderr: string;
  stdout_bytes: number;
  stderr_bytes: number;
  stdout_truncated: boolean;
  stderr_truncated: boolean;
  exit_code: number | null;
  signal: number | null;
}
const ended = (r: Run) =>
  ["exited", "stopped", "timed_out", "interrupted", "failed"].includes(r.state);
const states: Record<string, string> = {
  starting: "正在启动",
  running: "运行中",
  exited: "已退出",
  stopped: "已停止",
  timed_out: "超时，已回收",
  interrupted: "已记录中断",
  failed: "启动失败",
  unowned: "监管锁已释放",
};
export function ManagedWorkspace({
  adapters,
  writable,
  admin,
  executionAllowed,
  refreshSignal,
  onChange,
  onDirtyChange,
}: {
  adapters: Adapter[];
  writable: boolean;
  admin: boolean;
  executionAllowed: boolean;
  refreshSignal: unknown;
  onChange: () => Promise<void>;
  onDirtyChange?: (v: boolean) => void;
}) {
  const [agent, setAgent] = useState("codex"),
    [records, setRecords] = useState<Entity[]>([]),
    [mcpId, setMcpId] = useState(""),
    [selection, setSelection] = useState<Selection | null>(null),
    [plan, setPlan] = useState<Plan | null>(null),
    [runs, setRuns] = useState<Run[]>([]),
    [selectedRun, setSelectedRun] = useState(""),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [duration, setDuration] = useState(1000),
    [timeout, setTimeoutMs] = useState(5000);
  const generation = useRef(0);
  useUnsavedChanges(false, onDirtyChange);
  async function load(expectedGeneration = generation.current) {
    const [s, r, jobs] = await Promise.all([
      call<Selection>("deployment.inspect", { target_agent: agent }),
      call<{ entities: Entity[] }>("entity.list", { kind: "mcp" }),
      call<{ runs: Run[] }>("process.list"),
    ]);
    if (generation.current !== expectedGeneration) return;
    setSelection(s);
    setRecords(r.entities.filter((e) => !e.conflicted && !e.heads[0].deleted));
    setRuns(jobs.runs.filter((r) => r.target_agent === agent));
  }
  useEffect(() => {
    const n = ++generation.current;
    setSelection(null);
    setPlan(null);
    setSelectedRun("");
    setRuns([]);
    setError("");
    void load(n).catch((e) => {
      if (n === generation.current) setError(String(e.message));
    });
    return () => {
      generation.current++;
    };
  }, [agent, refreshSignal]);
  const active = runs.find((r) => !ended(r)),
    current = runs.find((r) => r.run_id === selectedRun) ?? runs[0];
  useEffect(() => {
    if (!active) return;
    const n = generation.current;
    const timer = window.setInterval(
      () =>
        void load(n).catch((e) => {
          if (generation.current === n) setError(String(e.message));
        }),
      700,
    );
    return () => window.clearInterval(timer);
  }, [agent, !!active, refreshSignal]);
  async function action(work: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await work();
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "操作失败，请刷新核对结果。");
    } finally {
      setBusy(false);
    }
  }
  const canWrite = writable && admin,
    canStart = canWrite && (executionAllowed || desktopAvailable);
  return (
    <div className="managed-workspace">
      <div className="section-heading">
        <div>
          <h1>受管配置与运行</h1>
          <p>预览生成文件，保留版本，再验证本机进程生命周期。</p>
        </div>
      </div>
      <div className="notice">
        <strong>当前只运行 Continuo 自带模拟程序</strong>
        <p>
          三个 agent 只决定生成配置的格式。这里不会启动真实 agent 或 MCP
          命令，不读取账号或个人配置。独立 home
          不提供文件、网络或系统钥匙串的安全隔离；输出和进程记录只留在本机。
        </p>
      </div>
      {error && (
        <div className="notice error" role="alert">
          {error}
        </div>
      )}
      <section className="settings-form">
        <h2>1. 生成配置</h2>
        <label>
          目标 agent
          <select
            value={agent}
            disabled={busy}
            onChange={(e) => {
              setAgent(e.target.value);
              setMcpId("");
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
          MCP 定义
          <select
            value={mcpId}
            disabled={busy || !selection}
            onChange={(e) => {
              setMcpId(e.target.value);
              setPlan(null);
            }}
          >
            <option value="">选择已有定义</option>
            {records.map((r) => (
              <option key={r.id} value={r.id}>
                {r.heads[0].name}
              </option>
            ))}
          </select>
        </label>
        {!records.length && (
          <p className="empty">
            先在 MCP 模块创建 stdio 定义并完成本机映射。这里不自动填充示例记录。
          </p>
        )}
        <p>
          配置中的命令需已存在、参数需已审查；凭据引用尚未解析时会阻止生成。只生成新
          home，不合并个人文件。
        </p>
        <button
          disabled={busy || !mcpId || !selection}
          onClick={() =>
            void action(async () => {
              setPlan(
                await call<Plan>("deployment.plan", {
                  target_agent: agent,
                  mcp_id: mcpId,
                }),
              );
            })
          }
        >
          预览配置文件
        </button>
        {plan && (
          <>
            <strong>{plan.file.relative_path}</strong>
            <pre aria-label="配置预览">{plan.file.content}</pre>
            <button
              className="primary"
              disabled={busy || !canWrite || !!active || !selection}
              onClick={() => {
                if (
                  !window.confirm(
                    "将刚预览的配置写入全新的 Continuo 受管 home，并切换本机选择？不复制账号或个人配置。",
                  )
                )
                  return;
                void action(async () => {
                  await call("deployment.apply", {
                    target_agent: agent,
                    mcp_id: mcpId,
                    expected_revision: selection!.expected_revision,
                    plan_digest: plan.plan_digest,
                    confirm_generated_home: true,
                  });
                  setPlan(null);
                  await onChange();
                });
              }}
            >
              确认投递配置
            </button>
          </>
        )}
        {selection?.active && (
          <>
            <p>
              当前选择：<code>{selection.expected_revision}</code>
            </p>
            <p>
              完整性：
              {selection.integrity?.ready
                ? "文件完整"
                : (selection.integrity?.error?.message ?? "无法验证")}
            </p>
            <code>{selection.integrity?.home}</code>
            <button
              disabled={
                busy || !canWrite || !!active || !selection.active.parent
              }
              onClick={() => {
                if (
                  !window.confirm(
                    "重新选择上一版完整 home？各版本文件保留，不迁移运行状态。",
                  )
                )
                  return;
                void action(async () => {
                  await call("deployment.rollback", {
                    target_agent: agent,
                    expected_revision: selection.expected_revision,
                    target_revision: selection.active!.parent,
                  });
                  await onChange();
                });
              }}
            >
              回滚上一选择
            </button>
          </>
        )}
        {!canWrite && <p>当前入口需要写入和管理权限才能投递或回滚。</p>}
        {!!active && (
          <p>存在尚未结束的运行，暂不能切换配置。先停止或检查监管状态。</p>
        )}
      </section>
      <section className="settings-form">
        <h2>2. 验证模拟运行</h2>
        <label>
          运行时长（毫秒）
          <input
            type="number"
            min={100}
            max={10000}
            value={duration}
            disabled={busy}
            onChange={(e) => setDuration(Number(e.target.value))}
          />
        </label>
        <label>
          超时（毫秒）
          <input
            type="number"
            min={100}
            max={15000}
            value={timeout}
            disabled={busy}
            onChange={(e) => setTimeoutMs(Number(e.target.value))}
          />
        </label>
        {!canStart && (
          <p>
            执行能力默认关闭。Web 需由用户在本机以 --allow-writes --allow-admin
            --allow-managed-processes 启动；页面不能自行扩大权限。
          </p>
        )}
        <button
          className="primary"
          disabled={
            busy || !canStart || !selection?.integrity?.ready || !!active
          }
          onClick={() => {
            if (
              !window.confirm(
                "确认只启动 Continuo 内置模拟程序？将使用当前生成 home、清空继承环境，按所填超时回收；不会启动真实 agent。",
              )
            )
              return;
            void action(async () => {
              const run = await call<Run>("process.start", {
                target_agent: agent,
                expected_revision: selection!.expected_revision,
                run_id: crypto.randomUUID(),
                confirm_simulation: true,
                duration_ms: duration,
                timeout_ms: timeout,
              });
              setSelectedRun(run.run_id);
            });
          }}
        >
          确认启动模拟程序
        </button>
      </section>
      <section className="settings-form">
        <h2>3. 状态与受限输出</h2>
        <button disabled={busy} onClick={() => void action(async () => {})}>
          刷新运行状态
        </button>
        {!runs.length ? (
          <p className="empty">本机尚无受管模拟运行。</p>
        ) : (
          <label>
            本机运行
            <select
              value={current?.run_id ?? ""}
              onChange={(e) => setSelectedRun(e.target.value)}
            >
              {runs.map((r) => (
                <option key={r.run_id} value={r.run_id}>
                  {states[r.observed_state]} · {r.run_id}
                </option>
              ))}
            </select>
          </label>
        )}
        {current && (
          <>
            <p role="status">
              {states[current.observed_state]} · 退出码{" "}
              {current.exit_code ?? "—"} · 信号 {current.signal ?? "—"}
            </p>
            <p>
              监管锁：{current.owner_present ? "持有" : "已释放"} · 子进程结束：
              {current.termination_verified ? "已确认并回收" : "未确认"}
            </p>
            {!ended(current) && !current.owner_present && (
              <p>
                可能仍在启动或监管者已退出。先刷新，再决定是否记录中断；恢复操作不会结束旧
                PID 或自动重启。
              </p>
            )}
            <button
              disabled={busy || !canWrite || ended(current)}
              onClick={() => {
                if (
                  !window.confirm(
                    "请求停止这一版运行？由持有运行锁的监管者结束自己创建的子进程。",
                  )
                )
                  return;
                void action(async () => {
                  await call("process.stop", {
                    run_id: current.run_id,
                    expected_control_revision: current.control_revision,
                    confirm_stop: true,
                  });
                });
              }}
            >
              停止此运行
            </button>
            <button
              disabled={
                busy || !canWrite || ended(current) || current.owner_present
              }
              onClick={() => {
                if (
                  !window.confirm(
                    "仅在监管锁已释放时记录中断，不结束或接管旧 PID，也不自动重启。确认？",
                  )
                )
                  return;
                void action(async () => {
                  await call("process.recover", {
                    run_id: current.run_id,
                    expected_control_revision: current.control_revision,
                    confirm_recovery: true,
                  });
                });
              }}
            >
              确认记录中断
            </button>
            <p>
              stdout：{current.stdout_bytes} 字节
              {current.stdout_truncated ? "（显示前 4096 字节）" : ""}
            </p>
            <pre aria-label="运行标准输出">{current.stdout || "暂无输出"}</pre>
            <p>
              stderr：{current.stderr_bytes} 字节
              {current.stderr_truncated ? "（显示前 4096 字节）" : ""}
            </p>
            <pre aria-label="运行错误输出">{current.stderr || "暂无输出"}</pre>
          </>
        )}
      </section>
    </div>
  );
}

import type { Plugin } from "@opencode-ai/plugin";

const SERVICE = "rust-git-guard";

// Можно переопределять командами окружения, если захочешь совпасть 1-в-1 с CI
const FMT_CMD =
  process.env.OPENCODE_RUST_GUARD_FMT ??
  "cargo fmt --all -- --check";

const CHECK_CMD =
  process.env.OPENCODE_RUST_GUARD_CHECK ??
  "cargo check --workspace --all-targets --locked";

const CLIPPY_CMD =
  process.env.OPENCODE_RUST_GUARD_CLIPPY ??
  "cargo clippy --workspace --all-targets --locked -- -D warnings";

const STEPS: Array<{ name: string; cmd: string }> = [
  { name: "cargo fmt (--check)", cmd: FMT_CMD },
  { name: "cargo check", cmd: CHECK_CMD },
  { name: "argo clippy --workspace --all-targets --all-features -- -D warnings", cmd: CLIPPY_CMD },
];

// Грубый, но практичный детектор "git ... commit|push" даже с опциями `-C`, `--git-dir`, и т.п.
function detectGitCommitOrPush(shellCmd: string): "commit" | "push" | null {
  const re =
    /\bgit\b(?:\s+(?:-[^\s]+|--[^\s]+|-(?:C)\s+\S+|--(?:git-dir|work-tree)\s+\S+))*\s+(commit|push)\b/m;
  const m = shellCmd.match(re);
  if (!m) return null;
  return m[1] === "commit" ? "commit" : "push";
}

async function capture(
  cwd: string,
  cmd: string,
): Promise<{ code: number; stdout: string; stderr: string }> {
  const proc = Bun.spawn({
    cmd: ["bash", "-lc", cmd],
    cwd,
    stdout: "pipe",
    stderr: "pipe",
    env: { ...process.env, CARGO_TERM_COLOR: "always" },
  });

  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);

  return { code, stdout: stdout.trim(), stderr: stderr.trim() };
}

async function runInherit(cwd: string, cmd: string): Promise<number> {
  const proc = Bun.spawn({
    cmd: ["bash", "-lc", cmd],
    cwd,
    stdout: "inherit",
    stderr: "inherit",
    env: { ...process.env, CARGO_TERM_COLOR: "always" },
  });
  return await proc.exited;
}

async function isGitRepo(root: string): Promise<boolean> {
  const r = await capture(root, "git rev-parse --is-inside-work-tree");
  return r.code === 0 && r.stdout === "true";
}

async function hasCargoToml(root: string): Promise<boolean> {
  return await Bun.file(`${root}/Cargo.toml`).exists();
}

// Важно: чтобы проверки относились к тому же коду, который попадёт в commit,
// запрещаем любые различия "worktree vs index" (unstaged / partial staged)
async function ensureNoUnstagedOrConflicts(root: string): Promise<{ ok: true } | { ok: false; reason: string }> {
  // Есть ли конфликты?
  const conflicts = await capture(root, "git diff --name-only --diff-filter=U");
  if (conflicts.code === 0 && conflicts.stdout.length > 0) {
    return {
      ok: false,
      reason: `В репозитории есть конфликтующие файлы (diff-filter=U):\n${conflicts.stdout}`,
    };
  }

  // Есть ли различия между worktree и index?
  const unstaged = await capture(root, "git diff --quiet; echo $?");
  // git diff --quiet => exit 0 если нет изменений, exit 1 если есть
  if (unstaged.code === 0 && unstaged.stdout !== "0") {
    return {
      ok: false,
      reason:
        "Есть незастейдженные изменения или partial-staged (worktree != index).\n" +
        "Сделай рабочее дерево эквивалентным index: `git add ...` (или откати/сташь), и повтори.",
    };
  }

  return { ok: true };
}

async function indexTreeHash(root: string): Promise<string | null> {
  // Детерминированный “отпечаток” того, что реально уйдёт в коммит (index tree).
  // Работает корректно, когда worktree==index (мы это отдельно проверяем).
  const r = await capture(root, "git write-tree");
  if (r.code !== 0) return null;
  return r.stdout || null;
}

export const RustGitGuardPlugin: Plugin = async ({ client, worktree }) => {
  // Кэш: если код (index tree) не менялся — повторно не гоняем проверки
  let lastOkTree: string | null = null;

  return {
    "tool.execute.before": async (input: any, output: any) => {
      const toolName = input?.tool;
      if (toolName !== "bash") return;

      const cmd: string | undefined = output?.args?.command ?? input?.args?.command;
      if (!cmd) return;

      const kind = detectGitCommitOrPush(cmd);
      if (!kind) return;

      const root = worktree || process.cwd();

      // Вне git — не мешаем
      if (!(await isGitRepo(root))) return;

      // Не Rust-проект — не мешаем
      if (!(await hasCargoToml(root))) return;

      // Валидация состояния для детерминизма
      const clean = await ensureNoUnstagedOrConflicts(root);
      if (!clean.ok) {
        await client.app.log({
          service: SERVICE,
          level: "warn",
          message: `Blocked git ${kind}: worktree/index not clean`,
          extra: { reason: clean.reason },
        });

        output.args.command =
          `echo "🛑 ${SERVICE}: блокирую git ${kind}." >&2; ` +
          `echo "" >&2; ` +
          `echo "${clean.reason.replace(/"/g, '\\"')}" >&2; ` +
          `echo "" >&2; ` +
          `echo "После этого прогоню: ${FMT_CMD} && ${CHECK_CMD} && ${CLIPPY_CMD}" >&2; ` +
          `exit 1`;
        return;
      }

      const tree = await indexTreeHash(root);
      if (tree && lastOkTree === tree) {
        // Ничего не менялось — разрешаем commit/push
        return;
      }

      await client.app.log({
        service: SERVICE,
        level: "info",
        message: `Running Rust checks before allowing git ${kind}`,
        extra: { steps: STEPS.map((s) => s.cmd) },
      });

      for (const step of STEPS) {
        // Маркер шага в выводе
        // (в TUI обычно видно stdout/stderr процессов)
        console.error(`\n[${SERVICE}] ▶ ${step.name}: ${step.cmd}\n`);

        const code = await runInherit(root, step.cmd);
        if (code !== 0) {
          await client.app.log({
            service: SERVICE,
            level: "warn",
            message: `Blocked git ${kind}: ${step.name} failed`,
            extra: { exitCode: code, cmd: step.cmd },
          });

          output.args.command =
            `echo "🛑 ${SERVICE}: блокирую git ${kind} — шаг '${step.name}' упал (exit ${code})." >&2; ` +
            `echo "Почини ошибки и повтори commit/push." >&2; ` +
            `exit 1`;
          return;
        }
      }

      // Все ок — запоминаем “снимок” index tree
      if (tree) lastOkTree = tree;

      await client.app.log({
        service: SERVICE,
        level: "info",
        message: `Allowed git ${kind}: all checks passed`,
        extra: { tree },
      });
    },
  };
};
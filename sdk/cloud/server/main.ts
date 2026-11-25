// raea cloud's server (docs/cloud.feature CL4-CL8): raea and an ACP agent behind one port and one token.
//
//   GET  /healthz                       "ok", no token (platform health checks)
//   GET  /v1/health                     {raea, agent, agents, workspace, device, offload, jobs}: device says whether
//                                       raea renders on a GPU here, and which (gpu.ts)
//   GET  /v1/files?path=P               a file in the workspace, or a folder's entries as JSON
//   PUT  /v1/files?path=P               writes the body to P, making its folders
//   POST /v1/run {args, cwd?}           `raea ARGS` in the workspace: NDJSON {out} and {err} lines, then {exit}
//   POST /v1/shell {command, cwd?, timeout_ms?}
//                                       a bash command line in the workspace, the same NDJSON; past its timeout
//                                       (10 min unless given, 1 h at most) its process group is stopped and the last
//                                       line says {exit, timed_out: true}. For an agent's shell tool (raea cloud)
//   POST /v1/login?agent=ID             an agent's terminal sign-in: {url, code, state, output}; GET says where it is
//   POST /v1/claude-token               a Claude subscription sign-in in a browser: {id, url} (claude-token.ts)
//   POST /v1/claude-token/ID {code}     the code claude.com showed, in: {token}, handed back once and never kept
//   POST /v1/claude-token/check {token} one short Claude Code turn on it: {ok, said}
//   POST /v1/google-token {screen?}     a Google AI subscription sign-in (Antigravity): {id, url, screen} (google-token.ts)
//   POST /v1/google-token/ID {address?} the address Google's sign-in landed on, in (none when it ran in the box's own
//                                       browser): {credential, email?}, handed back once, or {pending: true}
//   WS   /v1/google-token/ID/screen     the box's own browser for that sign-in, over VNC (google-screen.ts)
//   POST /v1/google-token/check {credential} one short Antigravity turn on it (or a Gemini API key): {ok, said, models}
//   WS   /v1/acp?agent=ID               one agent process per socket: a text frame is one ACP message to its stdin,
//                                       each line it prints is one frame back. A caller signing the agent in as one of
//                                       its own clients (raea cloud, per workspace) sends x-raea-agent-env: JSON of the
//                                       model keys below, and x-raea-agent-home: a short name. That socket's agent gets
//                                       only those keys (never the container's) and its own config folder under
//                                       ~/.agent-homes/NAME, so one client's sign-in never runs another's work. A
//                                       Google sign-in (RAEA_GOOGLE_CREDENTIAL) is the one thing written: its token
//                                       file, into that socket's own GEMINI_HOME, where Antigravity reads it.
//   POST /v1/jobs?args=JSON             a render job: a folder in (tar.zst), `raea ARGS` on it, followed with
//   GET  /v1/jobs/ID?from=N             short requests, and the files it made back from /out (jobs.ts); how a CPU
//   GET  /v1/jobs/ID/out                box hands renders to a GPU worker (offload.ts)
//
// Every route but /healthz needs RAEA_CLOUD_TOKEN: `Authorization: Bearer TOKEN`, or ?token= (a browser's WebSocket
// can't set headers). The agent and raea get the container's environment without the token or the state keys.

import { execFileSync, spawn } from "node:child_process"
import { timingSafeEqual } from "node:crypto"
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, symlinkSync, writeFileSync } from "node:fs"
import { createServer, type IncomingMessage, type ServerResponse } from "node:http"
import path from "node:path"
import { WebSocketServer } from "ws"
import { installed, spawnFor } from "./agents.ts"
import { guard } from "./memguard.ts"
import { device } from "./gpu.ts"
import * as jobs from "./jobs.ts"
import { checkClaude, finishClaudeSignIn, startClaudeSignIn } from "./claude-token.ts"
import { checkGoogle, finishGoogleSignIn, screenPort, startGoogleSignIn, unpackCredential } from "./google-token.ts"
import { connect as tcp } from "node:net"
import { login, loginState } from "./login.ts"
import { Outside, resolve, WORK } from "./workspace.ts"

const TOKEN = process.env.RAEA_CLOUD_TOKEN ?? ""
if (TOKEN.length < 16) {
  console.error("raea-cloud: set RAEA_CLOUD_TOKEN to a secret of 16 characters or more (openssl rand -hex 32); every request must carry it")
  process.exit(1)
}
const PORT = Number(process.env.PORT ?? 8080)
const RAEA = process.env.RAEA_BIN ?? "raea"

/** The environment raea and the agents run with: the container's, less this server's own secrets. */
const childEnv = (): NodeJS.ProcessEnv => {
  const env = { ...process.env }
  for (const k of Object.keys(env)) if (k === "RAEA_CLOUD_TOKEN" || k.startsWith("RAEA_CLOUD_STATE")) delete env[k]
  return env
}

let version: string | undefined

/** the model keys a caller may hand one agent process, and the config folder each agent keeps its state in */
const AGENT_KEYS = ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY", "CODEX_API_KEY", "GEMINI_API_KEY"] as const
const AGENT_HOMES: Record<string, string> = { CLAUDE_CONFIG_DIR: "claude", CODEX_HOME: "codex", GEMINI_CLI_HOME: "gemini", GEMINI_HOME: "gemini/.gemini" }
/** a Google sign-in's token file (google-token.ts), written into the socket's GEMINI_HOME rather than handed on */
const GOOGLE_CREDENTIAL = "RAEA_GOOGLE_CREDENTIAL"

/** the agent processes running for open sockets now (GET /v1/health running) */
const running = new Set<unknown>()

/** the box's build, as setup.sh wrote it (/opt/raea-cloud/BUILD.json); null on a box made before builds were stamped */
function buildOf(): Record<string, unknown> | null {
  try {
    return JSON.parse(readFileSync(path.join(process.env.RAEA_CLOUD_ROOT ?? "/opt/raea-cloud", "BUILD.json"), "utf8"))
  } catch {
    return null
  }
}

/** raea's skills, installed once in the box's own home (setup.sh), linked into a socket's own config folders: each
 *  client's agent gets its own home (AGENT_HOMES), which had left them without raea's skills (11-25) */
const SKILL_DIRS: Record<string, string> = { CLAUDE_CONFIG_DIR: ".claude/skills", GEMINI_HOME: ".gemini/skills" }
function linkSkills(env: Record<string, string>, home: string) {
  for (const [k, rel] of Object.entries(SKILL_DIRS)) {
    const from = path.join(home, rel)
    const to = path.join(env[k], "skills")
    try {
      if (existsSync(from) && !existsSync(to)) symlinkSync(from, to)
    } catch {
      // a folder of its own already there: left alone
    }
  }
}

/** a socket's own sign-in (its headers), or null to run on the container's environment */
export function socketEnv(headers: Record<string, string | string[] | undefined>, home = process.env.HOME ?? "/root"): Record<string, string> | null {
  const raw = headers["x-raea-agent-env"]
  if (typeof raw !== "string" || !raw) return null
  let given: Record<string, unknown>
  try {
    given = JSON.parse(raw)
  } catch {
    throw new Error("x-raea-agent-env isn't JSON")
  }
  const env: Record<string, string> = {}
  for (const k of AGENT_KEYS) {
    const v = given[k]
    if (typeof v === "string" && v) env[k] = v
  }
  const google = typeof given[GOOGLE_CREDENTIAL] === "string" ? (given[GOOGLE_CREDENTIAL] as string) : ""
  if (!Object.keys(env).length && !google) throw new Error(`x-raea-agent-env names none of ${[...AGENT_KEYS, GOOGLE_CREDENTIAL].join(", ")}`)
  const name = String(headers["x-raea-agent-home"] ?? "")
  if (!/^[a-z0-9][a-z0-9_-]{0,62}$/.test(name)) throw new Error("x-raea-agent-home must be a short lowercase name")
  for (const [k, dir] of Object.entries(AGENT_HOMES)) {
    env[k] = path.join(home, ".agent-homes", name, dir)
    mkdirSync(env[k], { recursive: true, mode: 0o700 })
  }
  linkSkills(env, home)
  if (google) {
    unpackCredential(google, env.GEMINI_HOME)
    env.AGY_ACP_FORCE_FILE_STORAGE = "1"
  }
  return env
}

/** what a socket signed in as one of raea cloud's clients runs with: its agent works one turn at a time for a caller
 *  that ends the run when the turn ends, so Claude Code mustn't background a command and end its turn to wait for it
 *  (11-21: a render backgrounded, the turn ended, the project stopped with nothing to show), and a render through
 *  raea's MCP tools or in the shell gets half an hour */
const RUN_DEFAULTS: Record<string, string> = { CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: "1", BASH_DEFAULT_TIMEOUT_MS: "1800000", BASH_MAX_TIMEOUT_MS: "3600000", MCP_TOOL_TIMEOUT: "1800000" }

/** the container's environment less every model key: a socket with its own sign-in never falls back to the box's */
const withoutKeys = (env: NodeJS.ProcessEnv) => {
  const out = { ...env }
  for (const k of [...AGENT_KEYS, "GOOGLE_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS"]) delete out[k]
  return out
}
const raeaVersion = () => {
  try {
    version ??= execFileSync(RAEA, ["--version"], { encoding: "utf8", timeout: 20_000, env: childEnv() }).trim()
  } catch {
    version = "unknown"
  }
  return version
}

function authorized(req: IncomingMessage, url: URL): boolean {
  const given = /^Bearer (.+)$/.exec(req.headers.authorization ?? "")?.[1] ?? url.searchParams.get("token") ?? ""
  const a = Buffer.from(given)
  const b = Buffer.from(TOKEN)
  return a.length === b.length && timingSafeEqual(a, b)
}

const json = (res: ServerResponse, v: unknown, status = 200) => res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(v))

async function body(req: IncomingMessage): Promise<Buffer> {
  const parts: Buffer[] = []
  for await (const c of req) parts.push(c as Buffer)
  return Buffer.concat(parts)
}

function files(req: IncomingMessage, res: ServerResponse, url: URL) {
  const p = resolve(url.searchParams.get("path") ?? ".")
  if (req.method === "PUT")
    return body(req).then((b) => {
      mkdirSync(path.dirname(p), { recursive: true })
      writeFileSync(p, b)
      json(res, { path: path.relative(WORK, p), bytes: b.length })
    })
  let st
  try {
    st = statSync(p)
  } catch {
    return json(res, { error: `${url.searchParams.get("path")} doesn't exist` }, 404)
  }
  if (st.isDirectory()) return json(res, readdirSync(p, { withFileTypes: true }).map((d) => ({ name: d.name, dir: d.isDirectory(), bytes: d.isFile() ? statSync(path.join(p, d.name)).size : undefined })))
  res.writeHead(200, { "content-type": "application/octet-stream", "content-length": st.size }).end(readFileSync(p))
}

async function run(req: IncomingMessage, res: ServerResponse) {
  const { args, cwd } = JSON.parse((await body(req)).toString() || "{}") as { args?: unknown; cwd?: string }
  if (!Array.isArray(args) || !args.every((a) => typeof a === "string")) return json(res, { error: "args must be an array of strings: raea's own arguments" }, 400)
  const dir = resolve(cwd ?? ".")
  mkdirSync(dir, { recursive: true })
  res.writeHead(200, { "content-type": "application/x-ndjson" })
  const child = spawn(RAEA, args, { cwd: dir, env: childEnv(), stdio: ["ignore", "pipe", "pipe"] })
  const line = (v: unknown) => res.write(JSON.stringify(v) + "\n")
  child.stdout.setEncoding("utf8").on("data", (out: string) => line({ out }))
  child.stderr.setEncoding("utf8").on("data", (err: string) => line({ err }))
  child.on("error", (e) => (line({ err: String(e) }), res.end(JSON.stringify({ exit: 127 }) + "\n")))
  child.on("close", (code, signal) => res.end(JSON.stringify({ exit: code ?? 128, signal: signal ?? undefined }) + "\n"))
  // the caller went away: the run stops with it
  res.on("close", () => child.exitCode === null && child.kill("SIGTERM"))
}

const SHELL_DEFAULT_MS = 600_000
const SHELL_MAX_MS = 3_600_000

async function shell(req: IncomingMessage, res: ServerResponse) {
  const { command, cwd, timeout_ms } = JSON.parse((await body(req)).toString() || "{}") as { command?: unknown; cwd?: string; timeout_ms?: unknown }
  if (typeof command !== "string" || !command.trim()) return json(res, { error: "command must be a bash command line" }, 400)
  const ms = typeof timeout_ms === "number" && timeout_ms > 0 ? Math.min(timeout_ms, SHELL_MAX_MS) : SHELL_DEFAULT_MS
  const dir = resolve(cwd ?? ".")
  mkdirSync(dir, { recursive: true })
  res.writeHead(200, { "content-type": "application/x-ndjson" })
  // the shell on any OS: bash and a process group here, RAEA_CLOUD_BASH and taskkill on Windows (shell-os.ts)
  const { bash, groupOpts, shellEnv, stopTree } = await import("./shell-os.ts")
  // in a process group of its own, so a timeout or a caller leaving stops what the command started too
  const child = spawn(bash(), ["-c", command], { cwd: dir, env: shellEnv(childEnv()), stdio: ["ignore", "pipe", "pipe"], ...groupOpts() })
  const line = (v: unknown) => res.write(JSON.stringify(v) + "\n")
  const stop = () => stopTree(child)
  let timedOut = false
  const timer = setTimeout(() => ((timedOut = true), stop()), ms)
  child.stdout.setEncoding("utf8").on("data", (out: string) => line({ out }))
  child.stderr.setEncoding("utf8").on("data", (err: string) => line({ err }))
  child.on("error", (e) => (clearTimeout(timer), line({ err: String(e) }), res.end(JSON.stringify({ exit: 127 }) + "\n")))
  child.on("close", (code, signal) => {
    clearTimeout(timer)
    res.end(JSON.stringify({ exit: code ?? 128, signal: signal ?? undefined, timed_out: timedOut || undefined }) + "\n")
  })
  res.on("close", () => (clearTimeout(timer), stop()))
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://raea-cloud")
  if (url.pathname === "/healthz") return res.writeHead(200, { "content-type": "text/plain" }).end("ok")
  if (!authorized(req, url)) return json(res, { error: "unauthorized" }, 401)
  try {
    if (url.pathname === "/v1/health" && req.method === "GET") {
      const def = (() => {
        try {
          return spawnFor().id
        } catch {
          return null
        }
      })()
      const gpu = process.env.RAEA_CLOUD_GPU_URL
      return json(res, {
        raea: raeaVersion(),
        // what deploy/sprites/box.sh put here: the build's id, the commit, whether the tree was dirty (BUILD.json)
        build: buildOf(),
        agent: def,
        agents: installed().map((a) => ({ id: a.id, version: a.version })),
        workspace: WORK,
        device: device(childEnv()),
        offload: gpu ? { url: gpu, commands: (process.env.RAEA_CLOUD_GPU_COMMANDS ?? "render").split(/\s+/).filter(Boolean) } : null,
        jobs: jobs.held(),
        // agents running now: deploy/sprites/box.sh up waits for none before it restarts this server
        running: running.size,
      })
    }
    if (url.pathname === "/v1/files" && (req.method === "GET" || req.method === "PUT")) return await files(req, res, url)
    if (url.pathname === "/v1/run" && req.method === "POST") return await run(req, res)
    if (url.pathname === "/v1/shell" && req.method === "POST") return await shell(req, res)
    if (url.pathname === "/v1/jobs" && req.method === "POST") {
      const args = jobs.readArgs(url.searchParams.get("args"))
      // a job runs raea here, never handed on again
      const env = childEnv()
      delete env.RAEA_CLOUD_GPU_URL
      delete env.RAEA_CLOUD_GPU_TOKEN
      return json(res, { id: jobs.start(args, await body(req), RAEA, env).id })
    }
    const job = /^\/v1\/jobs\/([0-9a-f-]{36})(\/out)?$/.exec(url.pathname)
    if (job && req.method === "GET") {
      if (job[2]) {
        const as = jobs.packing(url.searchParams.get("as"))
        const tar = jobs.out(job[1], as)
        if (!tar) return json(res, { error: "no finished job with that id" }, 404)
        const type = { zst: "application/zstd", gz: "application/gzip", tar: "application/x-tar" }[as]
        return res.writeHead(200, { "content-type": type, "content-length": tar.length }).end(tar)
      }
      const f = await jobs.follow(job[1], Number(url.searchParams.get("from") ?? 0))
      return f ? json(res, f) : json(res, { error: "no job with that id" }, 404)
    }
    if (url.pathname === "/v1/claude-token" && req.method === "POST") return json(res, await startClaudeSignIn(childEnv()))
    if (url.pathname === "/v1/claude-token/check" && req.method === "POST") {
      const b = JSON.parse((await body(req)).toString("utf8") || "{}") as { token?: unknown }
      return json(res, await checkClaude(childEnv(), String(b.token ?? "")))
    }
    if (url.pathname === "/v1/google-token" && req.method === "POST") {
      const b = JSON.parse((await body(req)).toString("utf8") || "{}") as { screen?: unknown }
      return json(res, await startGoogleSignIn(childEnv(), { screen: b.screen === true }))
    }
    if (url.pathname === "/v1/google-token/check" && req.method === "POST") {
      const b = JSON.parse((await body(req)).toString("utf8") || "{}") as { credential?: unknown }
      return json(res, await checkGoogle(childEnv(), String(b.credential ?? "")))
    }
    const gt = /^\/v1\/google-token\/([0-9a-f]{24})$/.exec(url.pathname)
    if (gt && req.method === "POST") {
      const b = JSON.parse((await body(req)).toString("utf8") || "{}") as { address?: unknown }
      return json(res, await finishGoogleSignIn(gt[1], typeof b.address === "string" && b.address.trim() ? b.address : undefined))
    }
    const ct = /^\/v1\/claude-token\/([0-9a-f]{24})$/.exec(url.pathname)
    if (ct && req.method === "POST") {
      const b = JSON.parse((await body(req)).toString("utf8") || "{}") as { code?: unknown }
      return json(res, await finishClaudeSignIn(ct[1], String(b.code ?? "")))
    }
    if (url.pathname === "/v1/login") {
      const a = spawnFor(url.searchParams.get("agent") ?? undefined)
      return json(res, req.method === "POST" ? await login(a, childEnv()) : loginState(a.id))
    }
    json(res, { error: `no route ${req.method} ${url.pathname}` }, 404)
  } catch (e) {
    if (e instanceof Outside) return json(res, { error: e.message }, 403)
    if (!res.headersSent) json(res, { error: (e as Error).message }, 400)
  }
})

// the WebSocket upgrade checks the token before a socket opens
const wss = new WebSocketServer({ noServer: true })
const agentSockets = new Set<import("ws").WebSocket>()
server.on("upgrade", (req, socket, head) => {
  const url = new URL(req.url ?? "/", "http://raea-cloud")
  // a Google sign-in's own browser (google-screen.ts): its VNC stream, both ways, as binary frames
  const gs = /^\/v1\/google-token\/([0-9a-f]{24})\/screen$/.exec(url.pathname)
  if (gs) {
    const port = authorized(req, url) ? screenPort(gs[1]) : undefined
    if (port === undefined) return socket.end(authorized(req, url) ? "HTTP/1.1 404 Not Found\r\n\r\n" : "HTTP/1.1 401 Unauthorized\r\n\r\n")
    return wss.handleUpgrade(req, socket, head, (ws) => {
      const vnc = tcp(port, "127.0.0.1")
      vnc.on("data", (d) => ws.readyState === ws.OPEN && ws.send(d))
      ws.on("message", (m) => vnc.write(m as Buffer))
      vnc.on("close", () => ws.close(1000, "the sign-in's screen closed"))
      vnc.on("error", () => ws.close(1011, "the sign-in's screen failed"))
      ws.on("close", () => vnc.destroy())
    })
  }
  if (url.pathname !== "/v1/acp" || !authorized(req, url)) return socket.end("HTTP/1.1 401 Unauthorized\r\n\r\n")
  let a
  try {
    a = spawnFor(url.searchParams.get("agent") ?? undefined)
  } catch (e) {
    return socket.end(`HTTP/1.1 400 Bad Request\r\ncontent-type: text/plain\r\n\r\n${(e as Error).message}`)
  }
  const agent = a
  let own: Record<string, string> | null
  try {
    own = socketEnv(req.headers)
  } catch (e) {
    return socket.end(`HTTP/1.1 400 Bad Request\r\ncontent-type: text/plain\r\n\r\n${(e as Error).message}`)
  }
  const env = own ? { ...withoutKeys(childEnv()), ...agent.env, ...RUN_DEFAULTS, ...own } : { ...childEnv(), ...agent.env }
  wss.handleUpgrade(req, socket, head, (ws) => {
    const child = spawn(agent.command, agent.args, { cwd: WORK, env, stdio: ["pipe", "pipe", "pipe"] })
    running.add(child)
    agentSockets.add(ws)
    ws.on("close", () => agentSockets.delete(ws))
    child.on("exit", () => running.delete(child))
    // its stderr to the box's log as before, and along the socket as _raea/stderr notifications, so raea cloud keeps
    // it with the run's trace (runtrace.ts) instead of only on this box's disk (11-25: raea-1's logs died with it)
    child.stderr.setEncoding("utf8").on("data", (c: string) => {
      process.stderr.write(c)
      if (ws.readyState === ws.OPEN) ws.send(JSON.stringify({ jsonrpc: "2.0", method: "_raea/stderr", params: { text: c.slice(0, 16_000) } }))
    })
    let buf = ""
    // ACP messages are newline-delimited JSON: split on \n only
    child.stdout.setEncoding("utf8").on("data", (c: string) => {
      buf += c
      for (let i; (i = buf.indexOf("\n")) >= 0; buf = buf.slice(i + 1)) if (i > 0) ws.send(buf.slice(0, i))
    })
    ws.on("message", (m) => child.stdin.write(m.toString() + "\n"))
    ws.on("close", () => child.exitCode === null && child.kill("SIGTERM"))
    child.on("error", (e) => ws.close(1011, `couldn't start ${agent.id}: ${e.message}`.slice(0, 120)))
    child.on("exit", (code) => ws.close(1011, `${agent.id} exited ${code}`))
  })
})

mkdirSync(WORK, { recursive: true })
// the box kept from running out of memory (memguard.ts): what it stops is said in the box's log and in every agent's
// trace (_raea/stderr), where a cut-off run's reason is read
if (existsSync("/proc/meminfo") && process.env.RAEA_CLOUD_MEMGUARD !== "0")
  guard((line) => {
    process.stderr.write(line + "\n")
    for (const ws of agentSockets) if (ws.readyState === ws.OPEN) ws.send(JSON.stringify({ jsonrpc: "2.0", method: "_raea/stderr", params: { text: line + "\n" } }))
  })
server.listen(PORT, () => {
  const d = device(childEnv())
  const gpu = process.env.RAEA_CLOUD_GPU_URL
  console.log(
    `raea-cloud: :${PORT}, workspace ${WORK}, agents ${installed().map((a) => a.id).join(", ") || "none"}, raea ${raeaVersion()}, renders on ${d.renders === "gpu" ? `the GPU (${d.name})` : `the CPU (${d.name})`}${gpu ? `, renders handed to ${gpu}` : ""}`,
  )
})

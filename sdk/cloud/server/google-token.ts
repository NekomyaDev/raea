// A Google AI subscription (Google One AI Pro or Ultra) for the box's Gemini agent, Google Antigravity (the ACP
// registry's antigravity-acp). Since 2026-06-18 Gemini CLI refuses personal Google sign-ins, and Antigravity is where
// Google sends them; it is also where Pro and Ultra get Gemini 4 Neon. Its "Log in with Google" (ACP method
// oauth-personal) is an OAuth sign-in whose address sends the browser back to 127.0.0.1 on the machine it runs on.
// Here that machine is the box, so the person signs in in their own browser, lands on a page that doesn't load (their
// computer has nothing on that port), and pastes that page's address back; the box hands it to Antigravity's own
// listener on 127.0.0.1, exactly as their browser would have.
//
// Off macOS Antigravity keeps its Google token as one JSON file under GEMINI_HOME. After a sign-in that file goes back
// to the caller once, as one string (agy1:BASE64 of {path under GEMINI_HOME: text}), and is never kept here: each
// sign-in runs in a folder of its own that's removed when it ends, and one left waiting is stopped after 15 minutes.
// An agent socket that's given the string (x-raea-agent-env RAEA_GOOGLE_CREDENTIAL, main.ts) has it written into its
// own GEMINI_HOME before Antigravity starts.
//   POST /v1/google-token                    {id, url}: a sign-in started, and the link to open
//   POST /v1/google-token/ID {address}       {credential, email?}: the address the browser landed on; or {error}
//   POST /v1/google-token/check {credential} {ok, said, models, current}: one short turn on a credential or a Gemini
//                                            API key (AIza…)
import { type ChildProcess, spawn } from "node:child_process"
import { randomBytes } from "node:crypto"
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { spawnFor } from "./agents.ts"

export const GOOGLE_AGENT = process.env.RAEA_GOOGLE_AGENT ?? "antigravity-acp"
const PREFIX = "agy1:"
const API_KEY = /^AIza[0-9A-Za-z_-]{30,}$/
/** what never reaches Antigravity from the container: its own Google sign-ins and keys */
const GOOGLE_ENV = ["GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS", "GOOGLE_CLOUD_PROJECT", "GOOGLE_GENAI_USE_VERTEXAI", "GEMINI_HOME"]

// ---- one Antigravity process, spoken to over ACP on its stdin and stdout ----
interface Agy {
  child: ChildProcess
  /** everything it printed, both streams (its sign-in link goes to either) */
  out: string
  ended: boolean
  request<T = any>(method: string, params: Record<string, unknown>, ms?: number): Promise<T>
  onUpdate?: (u: any) => void
}

function startAgy(env: NodeJS.ProcessEnv, home: string, cwd: string, own: Record<string, string> = {}): Agy {
  const a = spawnFor(GOOGLE_AGENT)
  const clean: NodeJS.ProcessEnv = { ...env, ...a.env }
  for (const k of GOOGLE_ENV) delete clean[k]
  Object.assign(clean, own, { GEMINI_HOME: home, AGY_ACP_FORCE_FILE_STORAGE: "1", BROWSER: "true", NO_BROWSER: "true" })
  const child = spawn(a.command, a.args, { cwd, env: clean, stdio: ["pipe", "pipe", "pipe"] })
  const pending = new Map<number, { ok: (v: any) => void; no: (e: Error) => void }>()
  let next = 1
  let buf = ""
  const agy: Agy = {
    child,
    out: "",
    ended: false,
    request(method, params, ms = 60_000) {
      if (agy.ended) return Promise.reject(new Error(`Antigravity has stopped: ${tail(agy.out)}`))
      const id = next++
      return new Promise((ok, no) => {
        const t = setTimeout(() => (pending.delete(id), no(new Error(`Antigravity didn't answer ${method} in ${ms / 1000} s`))), ms)
        pending.set(id, { ok: (v) => (clearTimeout(t), ok(v)), no: (e) => (clearTimeout(t), no(e)) })
        child.stdin!.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n")
      })
    },
  }
  child.stdout!.setEncoding("utf8").on("data", (c: string) => {
    agy.out += c
    buf += c
    for (let i; (i = buf.indexOf("\n")) >= 0; ) {
      const line = buf.slice(0, i).trim()
      buf = buf.slice(i + 1)
      if (!line.startsWith("{")) continue
      let m: any
      try {
        m = JSON.parse(line)
      } catch {
        continue
      }
      if (m.id !== undefined && m.method === undefined) {
        const p = pending.get(m.id)
        pending.delete(m.id)
        if (m.error) p?.no(new Error(m.error.message ?? JSON.stringify(m.error)))
        else p?.ok(m.result)
      } else if (m.method === "session/update") agy.onUpdate?.(m.params?.update)
      else if (m.id !== undefined) {
        // its requests to us: a permission is granted (a check asks for none), anything else refused
        const o = m.params?.options?.find((x: any) => x.kind === "allow_once") ?? m.params?.options?.[0]
        const reply = m.method === "session/request_permission" && o ? { result: { outcome: { outcome: "selected", optionId: o.optionId } } } : { error: { code: -32601, message: `no ${m.method} here` } }
        child.stdin!.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, ...reply }) + "\n")
      }
    }
  })
  child.stderr!.setEncoding("utf8").on("data", (c: string) => (agy.out += c))
  child.on("exit", () => {
    agy.ended = true
    for (const p of pending.values()) p.no(new Error(`Antigravity stopped: ${tail(agy.out)}`))
    pending.clear()
  })
  return agy
}

const initialize = (a: Agy) => a.request("initialize", { protocolVersion: 1, clientCapabilities: {}, clientInfo: { name: "raea-cloud", version: "1" } })
/** its last words, without its Python warnings or log prefixes */
const tail = (out: string) =>
  out
    .split("\n")
    .filter((l) => l.trim() && !/SyntaxWarning|^\s*[IW]\d{4} /.test(l))
    .slice(-3)
    .join(" ")
    .slice(-300)

/** the Google sign-in link Antigravity printed, and where it sends the browser back to */
export function linkIn(out: string): { url: string; port: number; state: string } | undefined {
  const m = /https:\/\/accounts\.google\.com\/o\/oauth2\/[^\s"']+/.exec(out)
  if (!m) return undefined
  const u = new URL(m[0])
  const back = new URL(u.searchParams.get("redirect_uri") ?? "")
  if (!/^(127\.0\.0\.1|localhost|\[::1\])$/.test(back.hostname)) return undefined
  return { url: m[0], port: Number(back.port), state: u.searchParams.get("state") ?? "" }
}

/** the address the person's browser landed on, checked against the sign-in it belongs to: the query to hand on */
export function landedQuery(address: string, s: { port: number; state: string }): string {
  let u: URL
  try {
    u = new URL(address.trim())
  } catch {
    throw new Error("paste the whole address of the page Google sent you to (it starts http://127.0.0.1)")
  }
  if (!/^(127\.0\.0\.1|localhost|\[::1\])$/.test(u.hostname) || Number(u.port) !== s.port) throw new Error("that address isn't the page this sign-in sent you to: sign in from the newest link")
  if (u.searchParams.get("error")) throw new Error(`Google said ${u.searchParams.get("error")}`)
  if (!u.searchParams.get("code")) throw new Error("that address has no code in it: copy it from the browser's address bar after you approve")
  if (u.searchParams.get("state") !== s.state) throw new Error("that address is from another sign-in: sign in from the newest link")
  return `${u.pathname}${u.search}`
}

// ---- the credential: the files under GEMINI_HOME that hold a refresh token, as one string ----
export function packCredential(home: string): string | undefined {
  const files: Record<string, string> = {}
  const walk = (dir: string) => {
    for (const d of readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, d.name)
      if (d.isDirectory()) walk(p)
      else if (d.isFile() && d.name.endsWith(".json") && statSync(p).size < 64_000) {
        const text = readFileSync(p, "utf8")
        if (/"refresh_token"\s*:/.test(text)) files[path.relative(home, p)] = text
      }
    }
  }
  walk(home)
  return Object.keys(files).length ? PREFIX + Buffer.from(JSON.stringify(files)).toString("base64") : undefined
}

/** writes a credential's files into an agent's GEMINI_HOME, 0600 under 0700 folders; each path stays inside it */
export function unpackCredential(credential: string, home: string): void {
  if (!credential.startsWith(PREFIX)) throw new Error("that isn't a Google sign-in from raea cloud")
  let files: Record<string, unknown>
  try {
    files = JSON.parse(Buffer.from(credential.slice(PREFIX.length), "base64").toString("utf8"))
  } catch {
    throw new Error("that Google sign-in can't be read")
  }
  for (const [rel, text] of Object.entries(files)) {
    const p = path.resolve(home, rel)
    if (typeof text !== "string" || !p.startsWith(path.resolve(home) + path.sep)) throw new Error("that Google sign-in names a file outside its folder")
    mkdirSync(path.dirname(p), { recursive: true, mode: 0o700 })
    writeFileSync(p, text, { mode: 0o600 })
  }
}

/** the account a credential is for, when its token file says */
const emailIn = (credential: string): string | undefined => /"email"\s*:\s*"([^"]+@[^"]+)"/.exec(Buffer.from(credential.slice(PREFIX.length), "base64").toString("utf8"))?.[1]

// ---- signing in ----
interface SignIn {
  agy: Agy
  dir: string
  home: string
  port: number
  state: string
  authed: boolean
  failed?: string
  timer: NodeJS.Timeout
}
const waiting = new Map<string, SignIn>()

function end(id: string) {
  const s = waiting.get(id)
  if (!s) return
  waiting.delete(id)
  clearTimeout(s.timer)
  if (s.agy.child.exitCode === null) s.agy.child.kill("SIGTERM")
  rmSync(s.dir, { recursive: true, force: true })
}

const until = async (ok: () => boolean, ms: number) => {
  for (let t = 0; t < ms && !ok(); t += 200) await new Promise((r) => setTimeout(r, 200))
}

export async function startGoogleSignIn(env: NodeJS.ProcessEnv): Promise<{ id: string; url: string }> {
  const dir = mkdtempSync(path.join(tmpdir(), "google-sign-in-"))
  const home = path.join(dir, ".gemini")
  mkdirSync(home, { mode: 0o700 })
  const agy = startAgy(env, home, dir)
  const id = randomBytes(12).toString("hex")
  try {
    await initialize(agy)
  } catch (e) {
    agy.child.kill("SIGTERM")
    rmSync(dir, { recursive: true, force: true })
    throw new Error(`Antigravity didn't start: ${(e as Error).message}`)
  }
  const s: SignIn = { agy, dir, home, port: 0, state: "", authed: false, timer: setTimeout(() => end(id), 15 * 60_000) }
  waiting.set(id, s)
  // authenticate answers only once the browser has come back: it waits for the whole sign-in
  agy.request("authenticate", { methodId: "oauth-personal" }, 15 * 60_000).then(
    () => (s.authed = true),
    (e: Error) => (s.failed = e.message),
  )
  await until(() => !!linkIn(agy.out) || !!s.failed || agy.ended, 30_000)
  const link = linkIn(agy.out)
  if (!link) {
    const why = s.failed ?? tail(agy.out)
    end(id)
    throw new Error(`Antigravity didn't print a Google sign-in link${why ? `: ${why}` : ""}`)
  }
  s.port = link.port
  s.state = link.state
  return { id, url: link.url }
}

export async function finishGoogleSignIn(id: string, address: string): Promise<{ credential: string; email?: string }> {
  const s = waiting.get(id)
  if (!s) throw new Error("that sign-in has ended (they last 15 minutes): start again")
  const q = landedQuery(address, s)
  // the browser's visit to Antigravity's listener, made from the box
  const r = await fetch(`http://127.0.0.1:${s.port}${q}`, { signal: AbortSignal.timeout(30_000) }).catch((e: Error) => ({ ok: false, status: 0, statusText: e.message }))
  await until(() => s.authed || !!s.failed || s.agy.ended, 60_000)
  const credential = s.authed ? packCredential(s.home) : undefined
  const why = s.failed ?? (r.ok ? tail(s.agy.out) : `its sign-in page answered ${r.status || r.statusText}`)
  console.log(`raea-cloud: google sign-in ${id.slice(0, 6)} ${credential ? "got a credential" : `no credential: ${why}`}`)
  end(id)
  if (!credential) throw new Error(`Google's sign-in didn't finish${why ? `: ${why}` : ""}`)
  return { credential, email: emailIn(credential) }
}

/** whether a credential (or a Gemini API key) gets an answer from Antigravity: one short turn in a folder of its own
 *  that's removed after; the models it offers that account, and the one it picked */
export async function checkGoogle(env: NodeJS.ProcessEnv, credential: string): Promise<{ ok: boolean; said: string; models: { id: string; name: string }[]; current?: string }> {
  const c = credential.trim()
  const key = API_KEY.test(c)
  if (!key && !c.startsWith(PREFIX)) throw new Error("that isn't a Google sign-in from raea cloud or a Gemini API key (AIza…)")
  const dir = mkdtempSync(path.join(tmpdir(), "google-check-"))
  const home = path.join(dir, ".gemini")
  mkdirSync(home, { mode: 0o700 })
  if (!key) unpackCredential(c, home)
  const agy = startAgy(env, home, dir, key ? { GEMINI_API_KEY: c } : {})
  let said = ""
  agy.onUpdate = (u) => {
    if (u?.sessionUpdate === "agent_message_chunk" && u.content?.type === "text") said += u.content.text ?? ""
  }
  const timer = setTimeout(() => agy.child.kill("SIGTERM"), 120_000)
  try {
    await initialize(agy)
    // with a good credential it signs in from the file; a link printed means Google wants a new sign-in
    const auth = agy.request("authenticate", { methodId: key ? "gemini-api-key" : "oauth-personal" }, 45_000)
    await Promise.race([auth, until(() => !!linkIn(agy.out), 45_000).then(() => linkIn(agy.out) && Promise.reject(new Error("Google wants this account signed in again")))])
    const s = await agy.request<{ sessionId: string; models?: { availableModels?: { modelId: string; name?: string }[]; currentModelId?: string } }>("session/new", { cwd: dir, mcpServers: [] })
    const models = (s.models?.availableModels ?? []).map((m) => ({ id: m.modelId, name: m.name ?? m.modelId }))
    await agy.request("session/prompt", { sessionId: s.sessionId, prompt: [{ type: "text", text: "Reply with just the word: ready" }] }, 90_000)
    said = said.replace(/\s+/g, " ").trim().slice(0, 400)
    return { ok: /ready/i.test(said), said, models, current: s.models?.currentModelId }
  } catch (e) {
    return { ok: false, said: (e as Error).message.slice(0, 400), models: [] }
  } finally {
    clearTimeout(timer)
    if (agy.child.exitCode === null) agy.child.kill("SIGTERM")
    rmSync(dir, { recursive: true, force: true })
  }
}

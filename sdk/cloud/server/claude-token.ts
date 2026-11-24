// A Claude subscription's token, made by Claude Code's own sign-in (`claude setup-token`) on this box, so a person
// signs in with nothing but a browser: raea cloud asks for a sign-in, shows them the claude.com link it prints, they
// sign in there and paste back the code Anthropic shows, and Claude Code prints the token (a year long, for Claude
// Code's use only). The token goes back to the caller once and is never kept here: each sign-in runs in a config
// folder of its own that's removed when it ends, and one left waiting is stopped after 15 minutes.
//   POST /v1/claude-token              {id, url}: a sign-in started, and the link to open
//   POST /v1/claude-token/ID {code}    {token}: the code pasted in; or {error}
//   POST /v1/claude-token/check {token} {ok, said}: one short turn on a token
import { type ChildProcess, spawn } from "node:child_process"
import { randomBytes } from "node:crypto"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"

interface SignIn {
  child: ChildProcess
  dir: string
  out: string
  url?: string
  token?: string
  ended: boolean
  timer: NodeJS.Timeout
}

const waiting = new Map<string, SignIn>()

/** a program on a terminal (200x60), its input from our stdin and its screen to our stdout. script(1) was here first,
 *  but Claude Code never took Enter after a long code through it; written to the terminal directly, it does */
const BRIDGE = `import os, pty, select, struct, fcntl, termios, sys
pid, fd = pty.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 60, 200, 0, 0))
while True:
    r, _, _ = select.select([fd, 0], [], [])
    if fd in r:
        try:
            b = os.read(fd, 65536)
        except OSError:
            break
        if not b:
            break
        os.write(1, b)
    if 0 in r:
        b = os.read(0, 65536)
        if not b:
            break
        os.write(fd, b)
os.kill(pid, 9) if os.waitpid(pid, os.WNOHANG) == (0, 0) else None
`
const CLAUDE = process.env.RAEA_CLAUDE_BIN ?? "claude"
const TOKEN = /sk-ant-oat01-[A-Za-z0-9_-]{20,}/

/** the text a terminal shows, with its escapes gone; OSC 8 links keep their address */
export const plain = (s: string) =>
  s
    .replace(/\x1b\]8;[^;]*;([^\x07\x1b]*)(?:\x07|\x1b\\)/g, " $1 ")
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "")

/** the sign-in link Claude Code printed */
export const urlIn = (out: string) => /https:\/\/claude\.(?:com|ai)\/\S*oauth\/authorize\?\S+/.exec(plain(out))?.[0]
/** the token it printed once the code was accepted (on one line: the terminal is 200 columns wide, room for it; 1000 broke Enter after a long code) */
export const tokenIn = (out: string) => TOKEN.exec(plain(out))?.[0]

function end(id: string) {
  const s = waiting.get(id)
  if (!s) return
  waiting.delete(id)
  clearTimeout(s.timer)
  if (s.child.exitCode === null) s.child.kill("SIGTERM")
  rmSync(s.dir, { recursive: true, force: true })
}

const until = async (s: SignIn, ok: () => boolean, ms: number) => {
  for (let t = 0; t < ms && !ok() && !s.ended; t += 200) await new Promise((r) => setTimeout(r, 200))
}

export async function startClaudeSignIn(env: NodeJS.ProcessEnv): Promise<{ id: string; url: string }> {
  const dir = mkdtempSync(path.join(tmpdir(), "claude-sign-in-"))
  const id = randomBytes(12).toString("hex")
  const clean: NodeJS.ProcessEnv = { ...env, CLAUDE_CONFIG_DIR: dir, BROWSER: "true", TERM: "xterm-256color" }
  for (const k of ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"]) delete clean[k]
  // Claude Code on a terminal of its own (BRIDGE), 200 columns: the token fits on one line (the link is read from its
  // OSC 8 address)
  const child = spawn("python3", ["-I", "-c", BRIDGE, CLAUDE, "setup-token"], { cwd: dir, env: clean, stdio: ["pipe", "pipe", "pipe"] })
  const s: SignIn = { child, dir, out: "", ended: false, timer: setTimeout(() => end(id), 15 * 60_000) }
  waiting.set(id, s)
  const read = (c: string) => {
    s.out += c
    s.url ??= urlIn(s.out)
    s.token ??= tokenIn(s.out)
  }
  child.stdout!.setEncoding("utf8").on("data", read)
  child.stderr!.setEncoding("utf8").on("data", read)
  child.on("exit", () => (s.ended = true))
  await until(s, () => !!s.url, 30_000)
  if (!s.url) {
    const why = plain(s.out).replace(/\s+/g, " ").trim().slice(-300)
    end(id)
    throw new Error(`Claude Code didn't print a sign-in link${why ? `: ${why}` : ""}`)
  }
  return { id, url: s.url }
}

export async function finishClaudeSignIn(id: string, code: string): Promise<{ token: string }> {
  const s = waiting.get(id)
  if (!s) throw new Error("that sign-in has ended (they last 15 minutes): start again")
  const c = code.trim()
  if (!/^[A-Za-z0-9_#.~-]{10,600}$/.test(c)) throw new Error("that doesn't look like the code claude.com showed")
  const from = s.out.length
  // the code as one paste, then Enter on its own a second later (Enter in the same write is taken as part of the paste)
  s.child.stdin!.write(c)
  await new Promise((r) => setTimeout(r, 1000))
  s.child.stdin!.write("\r")
  // done at the token, or as soon as Claude Code says the code was wrong
  await until(s, () => !!s.token || /OAuth error|Invalid code|expired/i.test(plain(s.out.slice(from))), 60_000)
  const token = s.token
  const said = plain(s.out.slice(from)).replace(/\s+/g, " ").trim()
  // what Claude Code said after the code, any token masked: the service log shows why a sign-in didn't finish
  console.log(`raea-cloud: claude sign-in ${id.slice(0, 6)} ${token ? "got a token" : "no token"}: ${JSON.stringify(s.out.slice(from).replace(/sk-ant-[A-Za-z0-9_-]{8,}/g, "sk-ant-…")).slice(-1500)}`)
  end(id)
  if (!token) throw new Error(`Claude Code didn't accept the code${said ? `: ${said.slice(-300)}` : ""}`)
  return { token }
}

/** whether a token (a subscription's, or an API key) gets an answer from Claude Code: one short turn, in a config folder
 *  of its own that's removed after. {ok, said} */
export async function checkClaude(env: NodeJS.ProcessEnv, token: string): Promise<{ ok: boolean; said: string }> {
  const t = token.trim()
  if (!/^sk-ant-(oat01|api03)-[A-Za-z0-9_-]{20,}$/.test(t)) throw new Error("that isn't a Claude token (sk-ant-oat01-…) or API key (sk-ant-api03-…)")
  const dir = mkdtempSync(path.join(tmpdir(), "claude-check-"))
  const clean: NodeJS.ProcessEnv = { ...env, CLAUDE_CONFIG_DIR: dir }
  delete clean.ANTHROPIC_API_KEY
  delete clean.CLAUDE_CODE_OAUTH_TOKEN
  clean[t.startsWith("sk-ant-oat01-") ? "CLAUDE_CODE_OAUTH_TOKEN" : "ANTHROPIC_API_KEY"] = t
  try {
    const child = spawn(CLAUDE, ["-p", "Reply with just the word: ready", "--max-turns", "1"], { cwd: dir, env: clean, stdio: ["ignore", "pipe", "pipe"] })
    let out = ""
    child.stdout.setEncoding("utf8").on("data", (c: string) => (out += c))
    child.stderr.setEncoding("utf8").on("data", (c: string) => (out += c))
    const code = await new Promise<number | null>((ok) => {
      const timer = setTimeout(() => child.kill("SIGTERM"), 90_000)
      child.on("exit", (c) => (clearTimeout(timer), ok(c)))
    })
    const said = plain(out).replace(/\s+/g, " ").trim().slice(0, 400)
    return { ok: code === 0 && /ready/i.test(said), said }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

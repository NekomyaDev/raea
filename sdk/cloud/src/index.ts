// The raea cloud client (docs/cloud.feature CL11): raea and an ACP agent on a server of your own, from any TypeScript
// with fetch and WebSocket (Node 22+, Bun, a browser).
//
//   const cloud = new RaeaCloud({ url: "https://raea.example.com", token: process.env.RAEA_CLOUD_TOKEN })
//   await cloud.health()                                  // {raea, agent, agents, workspace}
//   await cloud.write("card/edit.tsx", source)            // into the workspace
//   await cloud.run(["render", "edit.tsx", "-o", "card.mp4"], { cwd: "card" })
//   const s = await cloud.session()                       // the agent RAEA_AGENT names, over ACP
//   await s.prompt("Make card/edit.tsx a 3 s title card that says hello", (u) => console.log(u.sessionUpdate))
//   const mp4 = await cloud.read("card/card.mp4")
//   s.close()
//   const { exit, files } = await cloud.job(["render", "edit.tsx", "-o", "out/a.mp4"], projectTar)   // a render job:
//                                          // a project in as a tar, what the render made back as one (CL14)

import { Session, type Permission } from "./session.ts"

export { Session, type Permission, type Update } from "./session.ts"

export interface Health {
  raea: string
  agent: string | null
  agents: { id: string; version: string }[]
  workspace: string
  /** where raea renders on this server: a GPU (and which) or the CPU */
  device: { renders: "gpu" | "cpu"; name: string; gpus: { name: string; type: string; driver: string; vendor: string }[]; nvidia?: string[] }
  /** the GPU worker this server hands its renders to, when it has one */
  offload: { url: string; commands: string[] } | null
  jobs: { id: string; running: boolean }[]
}

/** A render farm's choices for one job (deploy/modal/farm.py): at most `gpus` GPUs at once, each a `card` (Modal's
 *  names: L4, L40S, ...), and a price over `maxUsd` (estimated from the first frames) stops it before the rest. */
export interface Farm {
  gpus?: number
  card?: string
  maxUsd?: number
  /** the job's media outputs stay on the farm for `media` to send to R2 (docs/cloud-render.feature CR23) */
  media?: boolean
}

/** A media file a farm job made, by its Git LFS object id; with `href`, where in R2 it goes (signed for it) */
export interface Media {
  path: string
  oid: string
  size: number
  href?: string
  header?: Record<string, string>
}

export interface JobLog {
  log: ({ out: string } | { err: string })[]
  next: number
  exit?: number
}

export interface Entry {
  name: string
  dir: boolean
  bytes?: number
}

export interface Login {
  agent: string
  method?: string
  url?: string
  code?: string
  state: "waiting" | "done" | "failed" | "none"
  output: string
}

/** Reads a /v1/run or /v1/shell answer: NDJSON {out} and {err} lines to onOutput, until the line with the exit code. */
async function follow(res: Response, onOutput: ((text: string, stream: "out" | "err") => void) | undefined, what: string): Promise<{ exit: number; timedOut: boolean }> {
  const reader = res.body!.pipeThrough(new TextDecoderStream()).getReader()
  let buf = ""
  for (;;) {
    const { value, done } = await reader.read()
    if (done) break
    buf += value
    for (let i; (i = buf.indexOf("\n")) >= 0; buf = buf.slice(i + 1)) {
      const m = JSON.parse(buf.slice(0, i)) as { out?: string; err?: string; exit?: number; timed_out?: boolean }
      if (m.out !== undefined) onOutput?.(m.out, "out")
      if (m.err !== undefined) onOutput?.(m.err, "err")
      if (m.exit !== undefined) return { exit: m.exit, timedOut: !!m.timed_out }
    }
  }
  throw new Error(`${what} ended without an exit code`)
}

export class RaeaCloud {
  readonly url: string
  private token: string
  private spriteToken?: string
  private ws?: Promise<string>

  /** `spriteToken`: the box is on a Sprite behind the Sprite's own auth (`auth sprite`), so that token goes in
   *  Authorization and the box's in ?token=, which the box takes too. (session() can't send it: a WebSocket has no
   *  headers to put it in, so ACP needs a Sprite with public auth.) */
  constructor(o: { url: string; token: string; spriteToken?: string }) {
    this.url = o.url.replace(/\/+$/, "")
    this.token = o.token
    this.spriteToken = o.spriteToken
  }

  /** A request to the box, its response as it comes, for a caller that reads the body itself (a stream of a file
   *  into storage); it throws unless the box answers 2xx. */
  async fetch(path: string, init: RequestInit = {}): Promise<Response> {
    const url = this.spriteToken ? `${this.url}${path}${path.includes("?") ? "&" : "?"}token=${encodeURIComponent(this.token)}` : this.url + path
    const res = await fetch(url, { ...init, headers: { authorization: `Bearer ${this.spriteToken ?? this.token}`, ...init.headers } })
    if (!res.ok) throw new Error(`${init.method ?? "GET"} ${path}: ${res.status} ${await res.text()}`)
    return res
  }

  health(): Promise<Health> {
    return this.fetch("/v1/health").then((r) => r.json())
  }

  async read(path: string): Promise<Uint8Array> {
    return new Uint8Array(await (await this.fetch(`/v1/files?path=${encodeURIComponent(path)}`)).arrayBuffer())
  }

  async text(path: string): Promise<string> {
    return new TextDecoder().decode(await this.read(path))
  }

  list(path = "."): Promise<Entry[]> {
    return this.fetch(`/v1/files?path=${encodeURIComponent(path)}`).then((r) => r.json())
  }

  async write(path: string, data: string | Uint8Array): Promise<void> {
    await this.fetch(`/v1/files?path=${encodeURIComponent(path)}`, { method: "PUT", body: typeof data === "string" ? data : new Blob([data as Uint8Array<ArrayBuffer>]) })
  }

  /** `raea ARGS` in the workspace (or `cwd` in it): resolves with its exit code; output goes to onOutput as it comes. */
  async run(args: string[], o: { cwd?: string; onOutput?: (text: string, stream: "out" | "err") => void; signal?: AbortSignal } = {}): Promise<number> {
    const res = await this.fetch("/v1/run", { method: "POST", body: JSON.stringify({ args, cwd: o.cwd }), headers: { "content-type": "application/json" }, signal: o.signal })
    return (await follow(res, o.onOutput, "raea's run")).exit
  }

  /** A bash command line in the workspace (or `cwd` in it): its exit code, and whether it was stopped at its
   *  timeout (the server's 10 minutes unless `timeoutMs`, an hour at most); output goes to onOutput as it comes. */
  async shell(command: string, o: { cwd?: string; timeoutMs?: number; onOutput?: (text: string, stream: "out" | "err") => void; signal?: AbortSignal } = {}): Promise<{ exit: number; timedOut: boolean }> {
    const res = await this.fetch("/v1/shell", { method: "POST", body: JSON.stringify({ command, cwd: o.cwd, timeout_ms: o.timeoutMs }), headers: { "content-type": "application/json" }, signal: o.signal })
    return follow(res, o.onOutput, "the shell command")
  }

  /** Starts `raea ARGS` on a folder sent as a tar (zstd, gzip or none), in a folder of its own on the server (a
   *  render job): its id. `farm` is for raea's render farm (deploy/modal/farm.py), which splits a render across GPUs;
   *  a single server ignores it. */
  async startJob(args: string[], folder: Uint8Array, signal?: AbortSignal, farm: Farm = {}): Promise<string> {
    const q = new URLSearchParams({ args: JSON.stringify(args) })
    if (farm.gpus !== undefined) q.set("gpus", String(farm.gpus))
    if (farm.card) q.set("gpu", farm.card)
    if (farm.maxUsd !== undefined) q.set("max_usd", String(farm.maxUsd))
    if (farm.media) q.set("media", "1")
    const r = await this.fetch(`/v1/jobs?${q}`, { method: "POST", body: new Blob([folder as Uint8Array<ArrayBuffer>]), headers: { "content-type": "application/octet-stream" }, signal })
    return ((await r.json()) as { id: string }).id
  }

  /** A job's output since line `from`; the server holds the request up to 20 s for more. `exit` once it's done. */
  followJob(id: string, from = 0, signal?: AbortSignal): Promise<JobLog> {
    return this.fetch(`/v1/jobs/${id}?from=${from}`, { signal }).then((r) => r.json())
  }

  /** A finished job's files: the ones its run made or changed, as a tar (zstd unless `as` says gz or tar). The server
   *  drops the job after. */
  async jobFiles(id: string, as: "zst" | "gz" | "tar" = "zst"): Promise<Uint8Array> {
    // a farm whose volume is busy answers 503 and keeps the job (11-25: a 500 there lost a finished render): ask again
    for (let i = 0; ; i++) {
      try {
        return new Uint8Array(await (await this.fetch(`/v1/jobs/${id}/out?as=${as}`)).arrayBuffer())
      } catch (e) {
        if (i >= 4 || !/: 503 /.test((e as Error).message)) throw e
        await new Promise((r) => setTimeout(r, 1000 * 2 ** i))
      }
    }
  }

  /** A finished job's media outputs (a job started with farm.media), held on the farm until sendMedia. */
  async jobMedia(id: string): Promise<Media[]> {
    return ((await (await this.fetch(`/v1/jobs/${id}/media`)).json()) as { objects: Media[] }).objects
  }

  /** Sends a job's media outputs to R2: each with an href goes there (a signed PUT), straight from the farm. */
  async sendMedia(id: string, objects: Media[]): Promise<{ uploaded: number; bytes: number }> {
    const r = await this.fetch(`/v1/jobs/${id}/media`, { method: "POST", body: JSON.stringify({ objects }), headers: { "content-type": "application/json" } })
    return r.json()
  }

  /** A job from start to end: its output to onOutput as it comes; its exit code and the files it made or changed. A
   *  dropped follow request (a platform's proxy, a cold start) is tried again, up to five times in a row. With
   *  `media` (and farm.media), the job's media outputs are handed to it first: it answers where in R2 each goes, the
   *  farm sends them there, and they come back in `media`, not in `files`. */
  async job(args: string[], folder: Uint8Array, o: { onOutput?: (text: string, stream: "out" | "err") => void; signal?: AbortSignal; as?: "zst" | "gz" | "tar"; farm?: Farm; media?: (m: Media[]) => Promise<Media[]> } = {}): Promise<{ exit: number; files: Uint8Array; media: Media[] }> {
    const id = await this.startJob(args, folder, o.signal, o.farm)
    let from = 0
    for (let misses = 0; ; ) {
      let f: JobLog
      try {
        f = await this.followJob(id, from, o.signal)
        misses = 0
      } catch (e) {
        if (o.signal?.aborted || ++misses > 5) throw e
        await new Promise((ok) => setTimeout(ok, 1000 * misses))
        continue
      }
      for (const l of f.log) "out" in l ? o.onOutput?.(l.out, "out") : o.onOutput?.(l.err, "err")
      from = f.next
      if (f.exit === undefined) continue
      let media: Media[] = []
      if (o.farm?.media && o.media) {
        const made = await this.jobMedia(id)
        if (made.length) {
          media = await o.media(made)
          await this.sendMedia(id, media)
        }
      }
      return { exit: f.exit, files: await this.jobFiles(id, o.as), media }
    }
  }

  /** Starts an agent's terminal sign-in (Kimi Code's device code): open `url` and enter `code` once; the sign-in is
   *  kept in the server's home folder. */
  login(agent?: string): Promise<Login> {
    return this.fetch(`/v1/login${agent ? `?agent=${encodeURIComponent(agent)}` : ""}`, { method: "POST" }).then((r) => r.json())
  }

  loginState(agent?: string): Promise<Login> {
    return this.fetch(`/v1/login${agent ? `?agent=${encodeURIComponent(agent)}` : ""}`).then((r) => r.json())
  }

  /** An ACP session with an agent in the workspace (the one named, else the one the box's RAEA_AGENT names; with neither, the box refuses). */
  async session(o: { agent?: string; cwd?: string; onPermission?: (p: Permission) => string | Promise<string> } = {}): Promise<Session> {
    const work = (this.ws ??= this.health().then((h) => h.workspace))
    const q = new URLSearchParams({ token: this.token, ...(o.agent ? { agent: o.agent } : {}) })
    const ws = new WebSocket(`${this.url.replace(/^http/, "ws")}/v1/acp?${q}`)
    await new Promise((ok, no) => {
      ws.addEventListener("open", ok)
      ws.addEventListener("error", () => no(new Error(`couldn't open the ACP socket at ${this.url}`)))
    })
    const s = new Session(ws, o.onPermission)
    await s.request("initialize", { protocolVersion: 1, clientCapabilities: {} })
    const root = await work
    const cwd = !o.cwd ? root : o.cwd.startsWith("/") ? o.cwd : `${root}/${o.cwd}`
    const { sessionId } = (await s.request("session/new", { cwd, mcpServers: [] })) as { sessionId: string }
    s.id = sessionId
    return s
  }
}

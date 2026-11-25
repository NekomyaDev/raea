// The box's memory, kept from running out (11-25): StreetTalk's agent, its renders failing on the farm, found raea's
// own binary and rendered a cut of 4K interviews on the box's CPU; the 8 GB box ran out and stopped answering, every
// agent's socket with it. A box is a VM whose processes share one cgroup that can't be split, so there's no kernel
// cap to set: like earlyoom, this looks at the box's free memory every second and, when it runs low, kills the
// largest thing the box's agents started (a render, an ffmpeg), never the server and never an agent itself (they're
// a few hundred MB; only a process over MIN_RSS is a candidate). The command it was part of fails, said in its
// output by the shell ("Killed"), and the agent carries on.
import { readdirSync, readFileSync } from "node:fs"

/** below this much available memory (the larger of the two) the guard acts */
export const FLOOR_MB = 700
export const FLOOR_SHARE = 0.08
/** a process smaller than this is never killed: the agents, the server, a shell */
export const MIN_RSS_MB = 1024

export interface Proc {
  pid: number
  ppid: number
  rssMb: number
  name: string
}

/** every process on the box (Linux /proc) */
export function procs(root = "/proc"): Proc[] {
  const out: Proc[] = []
  for (const d of readdirSync(root)) {
    if (!/^\d+$/.test(d)) continue
    try {
      const stat = readFileSync(`${root}/${d}/stat`, "utf8")
      // the name is in parentheses and may hold spaces: the fields after its last ")" are fixed
      const close = stat.lastIndexOf(")")
      const name = stat.slice(stat.indexOf("(") + 1, close)
      const f = stat.slice(close + 2).split(" ")
      const status = readFileSync(`${root}/${d}/status`, "utf8")
      const rss = /VmRSS:\s+(\d+) kB/.exec(status)
      out.push({ pid: Number(d), ppid: Number(f[1]), rssMb: rss ? Number(rss[1]) / 1024 : 0, name })
    } catch {} // gone since the listing
  }
  return out
}

/** the box's memory: total and available, in MB */
export function memory(file = "/proc/meminfo"): { totalMb: number; availableMb: number } {
  const m = readFileSync(file, "utf8")
  const kb = (k: string) => Number(new RegExp(`${k}:\\s+(\\d+) kB`).exec(m)?.[1] ?? 0)
  return { totalMb: kb("MemTotal") / 1024, availableMb: kb("MemAvailable") / 1024 }
}

/** the process to kill when memory is low: the largest of `root`'s descendants over MIN_RSS_MB, or none */
export function victim(all: Proc[], root: number, keep: Set<number> = new Set()): Proc | null {
  const kids = new Map<number, Proc[]>()
  for (const p of all) kids.set(p.ppid, [...(kids.get(p.ppid) ?? []), p])
  const under: Proc[] = []
  const walk = (pid: number) => {
    for (const k of kids.get(pid) ?? []) {
      under.push(k)
      walk(k.pid)
    }
  }
  walk(root)
  const big = under.filter((p) => p.rssMb >= MIN_RSS_MB && !keep.has(p.pid)).sort((a, b) => b.rssMb - a.rssMb)
  return big[0] ?? null
}

export interface Guard {
  stop(): void
}

/** watch the box's memory and kill the largest runaway under `root` (this server) when it runs low; `said` hears why */
export function guard(said: (line: string) => void, o: { root?: number; everyMs?: number; keep?: () => Set<number>; mem?: typeof memory; list?: typeof procs; kill?: (pid: number) => void } = {}): Guard {
  const root = o.root ?? process.pid
  const mem = o.mem ?? memory
  const list = o.list ?? procs
  const kill = o.kill ?? ((pid: number) => process.kill(pid, "SIGKILL"))
  const t = setInterval(() => {
    try {
      const m = mem()
      if (m.availableMb >= Math.max(FLOOR_MB, m.totalMb * FLOOR_SHARE)) return
      const v = victim(list(), root, o.keep?.())
      if (!v) return
      kill(v.pid)
      said(`raea cloud: the box was running out of memory (${Math.round(m.availableMb)} MB of ${Math.round(m.totalMb)} MB left), so ${v.name} (pid ${v.pid}, ${(v.rssMb / 1024).toFixed(1)} GB) was stopped. Render on the GPU with raea render, not on this box`)
    } catch (e) {
      said(`raea cloud: the memory guard: ${(e as Error).message}`)
    }
  }, o.everyMs ?? 1000)
  t.unref()
  return { stop: () => clearInterval(t) }
}

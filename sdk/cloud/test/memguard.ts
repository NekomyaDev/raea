// The box's memory guard (server/memguard.ts): node test/memguard.ts from sdk/cloud. Offline, on a made-up process
// table; and on Linux, against a real process that grows past the floor (set high for the test), which it kills.
import { spawn } from "node:child_process"
import { existsSync } from "node:fs"
import { guard, MIN_RSS_MB, type Proc, victim } from "../server/memguard.ts"

const ok = (m: string) => console.log(`ok ${m}`)
const assert = (c: unknown, m: string) => {
  if (!c) throw new Error(`not ok: ${m}`)
}

// the server (1), an agent (2) and its helper (3), a shell (4) running a render (5) and an ffmpeg (6); another box
// process (9) outside the server
const table: Proc[] = [
  { pid: 1, ppid: 0, rssMb: 120, name: "node" },
  { pid: 2, ppid: 1, rssMb: 300, name: "agy_acp_server" },
  { pid: 3, ppid: 2, rssMb: 180, name: "localharness" },
  { pid: 4, ppid: 3, rssMb: 4, name: "bash" },
  { pid: 5, ppid: 4, rssMb: 5200, name: "raea" },
  { pid: 6, ppid: 4, rssMb: 1500, name: "ffmpeg" },
  { pid: 9, ppid: 0, rssMb: 9000, name: "other" },
]
assert(victim(table, 1)?.pid === 5, "the largest runaway under the server")
assert(victim(table, 1, new Set([5]))?.pid === 6, "a kept one is passed over")
assert(victim(table.filter((p) => p.pid !== 5 && p.pid !== 6), 1) === null, `nothing under ${MIN_RSS_MB} MB is killed: not an agent, not the server`)
ok("it picks the largest process an agent started, never the agent, the server or anything outside it")

// the loop: memory fine, nothing happens; memory low, the victim is killed and said
let avail = 4000
const killed: number[] = []
const said: string[] = []
const g = guard((l) => said.push(l), { root: 1, everyMs: 10, mem: () => ({ totalMb: 8192, availableMb: avail }), list: () => table, kill: (p) => killed.push(p) })
await new Promise((r) => setTimeout(r, 60))
assert(killed.length === 0, "memory fine: nothing killed")
avail = 500
await new Promise((r) => setTimeout(r, 30))
g.stop()
assert(killed[0] === 5 && /raea \(pid 5, 5\.1 GB\)/.test(said[0]), `low: the render is stopped and said: ${said[0]}`)
ok("memory low: the render is stopped, and the guard says why")

if (existsSync("/proc/meminfo")) {
  // a real child that grows to 1.5 GB, under a floor set just below what's free now, so its growth trips it
  const { memory } = await import("../server/memguard.ts")
  const start = memory().availableMb
  const hog = spawn(process.execPath, ["-e", "const a=[]; for (let i=0;i<150;i++) a.push(Buffer.alloc(10<<20, 1)); setInterval(()=>{}, 1000)"], { stdio: "ignore" })
  const lines: string[] = []
  const real = guard((l) => lines.push(l), { everyMs: 200, mem: () => { const m = memory(); return { totalMb: m.totalMb, availableMb: m.availableMb + 700 - (start - 1200) } } })
  const code = await new Promise<number | null>((r) => {
    hog.on("exit", (c, sig) => r(sig === "SIGKILL" ? -9 : c))
    setTimeout(() => (hog.kill("SIGKILL"), r(null)), 30_000)
  })
  real.stop()
  assert(code === -9 && lines.length === 1, `the hog was killed by the guard: ${code} ${lines}`)
  ok(`on this box: a 1.5 GB runaway was stopped (${lines[0].slice(0, 90)}…)`)
}

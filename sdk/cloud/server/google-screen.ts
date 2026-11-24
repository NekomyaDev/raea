// The Google sign-in's own browser (11-24, Shane: "I don't want people having to do it themselves"). Antigravity's
// Google sign-in sends the browser back to 127.0.0.1 on the machine it runs on, so a browser on the person's computer
// lands on a page that can't load. Here the sign-in runs in a Chrome on the box instead, on a virtual display of its
// own, and the person sees and works it from the dashboard over VNC (RFB, carried on a WebSocket: main.ts
// /v1/google-token/ID/screen). Google's last step then lands on Antigravity's listener on the box and the sign-in
// finishes by itself, 2-step verification and all (checked 11-24 on Shane's account).
//
// Each sign-in gets its own display, Chrome profile (in the sign-in's folder, so the Google session it holds is removed
// with it) and VNC port; all three stop when the sign-in ends. With no Chrome installed there's no screen, and the
// sign-in falls back to the pasted address.
import { type ChildProcess, spawn, spawnSync } from "node:child_process"
import { existsSync, readFileSync, rmSync } from "node:fs"
import { connect } from "node:net"
import path from "node:path"

/** the screen's size: Google's sign-in fits it, and the dashboard's panel scales it */
export const SCREEN = { width: 960, height: 760 }
const CHROMES = ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]

export interface Screen {
  /** the VNC server's port on 127.0.0.1 */
  port: number
  /** whether it's still up */
  up(): boolean
  stop(): void
}

const which = (bin: string) => spawnSync("sh", ["-c", `command -v ${bin}`], { encoding: "utf8" }).stdout.trim()
const chrome = () => process.env.RAEA_CHROME || CHROMES.map(which).find(Boolean) || ""
/** whether this box can show a sign-in: Chrome, a virtual display and a VNC server */
export const canShow = () => !!chrome() && !!which("Xvfb") && !!which("x11vnc")

const until = async (ok: () => boolean, ms: number) => {
  for (let t = 0; t < ms && !ok(); t += 100) await new Promise((r) => setTimeout(r, 100))
  return ok()
}
const listening = (port: number) =>
  new Promise<boolean>((ok) => {
    const s = connect(port, "127.0.0.1")
    s.once("connect", () => (s.destroy(), ok(true)))
    s.once("error", () => ok(false))
  })

const alive = (pid: number) => {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}
/** the first display number from 100 that's free: no X server holds it (a socket and lock left by one that's gone,
 *  as a restart of the service leaves them, are cleared) */
function freeDisplay(): number {
  for (let n = 100; n < 400; n++) {
    const sock = `/tmp/.X11-unix/X${n}`, lock = `/tmp/.X${n}-lock`
    const pid = existsSync(lock) ? Number(readFileSync(lock, "utf8").trim()) : 0
    if (pid && alive(pid)) continue
    if (existsSync(sock) || existsSync(lock)) {
      try {
        rmSync(sock, { force: true })
        rmSync(lock, { force: true })
      } catch {
        continue
      }
    }
    return n
  }
  throw new Error("no free display for the sign-in's browser")
}

/** whether Chrome has a window up on display n (xdotool; without it, assumed so) */
const windowUp = (n: number) => {
  if (!which("xdotool")) return true
  const r = spawnSync("xdotool", ["search", "--onlyvisible", "--class", "chrome"], { env: { ...process.env, DISPLAY: `:${n}` }, encoding: "utf8" })
  return r.status === 0 && r.stdout.trim() !== ""
}

/** a Chrome on a display of its own at url, served over VNC on 127.0.0.1; its profile is under dir */
export async function openScreen(dir: string, url: string): Promise<Screen> {
  const bin = chrome()
  if (!bin) throw new Error("no Chrome on this box")
  const n = freeDisplay()
  const port = 5900 + n
  const kids: ChildProcess[] = []
  const stop = () => {
    for (const k of kids.reverse()) if (k.exitCode === null && k.signalCode === null) k.kill("SIGTERM")
  }
  try {
    const x = spawn("Xvfb", [`:${n}`, "-screen", "0", `${SCREEN.width}x${SCREEN.height}x24`, "-nolisten", "tcp"], { stdio: "ignore" })
    kids.push(x)
    if (!(await until(() => existsSync(`/tmp/.X11-unix/X${n}`) || x.exitCode !== null, 10_000)) || x.exitCode !== null) throw new Error("the virtual display didn't start")
    const env = { ...process.env, DISPLAY: `:${n}` }
    // no sandbox: the box's container has no user namespaces for it (test-type keeps Chrome from warning about that in
    // a bar across Google's page; pages can't see either flag); the window is an app window (no tabs or address
    // bar), the whole screen, and keeps its passwords in its profile (no keyring to ask about)
    kids.push(
      spawn(
        bin,
        [
          "--no-sandbox",
          "--test-type",
          "--disable-gpu",
          "--disable-dev-shm-usage",
          "--no-first-run",
          "--no-default-browser-check",
          "--password-store=basic",
          `--user-data-dir=${path.join(dir, "chrome")}`,
          "--window-position=0,0",
          `--window-size=${SCREEN.width},${SCREEN.height}`,
          `--app=${url}`,
        ],
        { env, stdio: "ignore" },
      ),
    )
    kids.push(spawn("x11vnc", ["-display", `:${n}`, "-localhost", "-nopw", "-forever", "-shared", "-quiet", "-rfbport", String(port)], { env, stdio: "ignore" }))
    let ok = false
    for (let t = 0; t < 10_000 && !ok; t += 200) {
      ok = await listening(port)
      if (!ok) await new Promise((r) => setTimeout(r, 200))
    }
    if (!ok) throw new Error("the sign-in's screen didn't start")
    // the page, not a black screen: a fresh Chrome profile takes a while on the box's CPU (about 20 s seen 11-24)
    for (let t = 0; t < 45_000 && !windowUp(n); t += 300) await new Promise((r) => setTimeout(r, 300))
  } catch (e) {
    stop()
    throw e
  }
  return { port, up: () => kids.every((k) => k.exitCode === null && k.signalCode === null), stop }
}

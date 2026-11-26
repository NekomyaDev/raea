// `raea frames check` on a raea cloud box: what an export's pictures get wrong that a viewer sees at once, measured
// (11-25, Shane: "it cuts out faces all the time and also does split frame shots when they make no sense", "it makes
// text too small on some things, we should have a text/glyph readability check and measurement").
//
//   raea frames check VIDEO [--manifest RENDER.json] [-o REPORT.json]
//       faces: VIDEO's frames, two a second, through the farm's face detector (POST /v1/faces, OpenCV's YuNet; a box has
//       no vision backend of its own). A face of a person in the shot (at least 6% of the frame's height) touching an
//       edge is cut by the frame: for a second or more it's an error, shorter a warning.
//       text: each text the render's manifest shows (raea render --manifest; a .tsx render through raea cloud writes
//       VIDEO.render.json by itself), its font size in pixels of a 1080-wide frame: under 40 px is an error (no one
//       reads it on a phone), under 48 px a warning (small for anything but a badge or small print).
//   Exit 1 on any error. A face at the edge that's meant (someone walking out of shot) is said, with its reason, in
//   VIDEO.despite.txt beside the video: its findings are then warnings.
import { spawnSync } from "node:child_process"
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { pathToFileURL } from "node:url"

/** frames sampled a second, and the width they're read at (a face's place is in shares of the picture) */
const RATE = 2
const SIDE = 540
/** a face this share of the frame's height or more is someone in the shot, not a passer-by far behind (a split
 *  screen's half holds a face about this big) */
export const FACE_MIN = 0.05
/** least font sizes, in pixels of a frame 1080 wide (its shorter side): the Direct Response skill's table */
export const TEXT_ERROR = 40
export const TEXT_WARN = 48

export type Finding = { level: "error" | "warn"; kind: "face-cut" | "split" | "text-small"; at: number | null; until?: number; says: string }
/** a line where the picture jumps across nearly the whole frame (the farm's): its axis (h, a row), where, and its share */
export type Seam = [string, number, number]
export type Faces = { w: number; h: number; faces: number[][]; seams?: Seam[] } | null
type Split = { axis: "h" | "v"; at: number }

/** a frame's split-screen seams: lone lines in its middle 60% (a bar's two edges close together are a caption's box, a
 *  badge or a letterbox, not a seam) */
export function splitsOf(fr: NonNullable<Faces>): Split[] {
  const out: Split[] = []
  for (const axis of ["h", "v"] as const) {
    const dim = axis === "h" ? fr.h : fr.w
    const at = [...new Set((fr.seams ?? []).filter((x) => x[0] === axis).map((x) => x[1]))].sort((a, b) => a - b)
    const lines = at.filter((y, i) => i === 0 || y - at[i - 1] > 4)
    for (const y of lines) {
      const bar = lines.some((o) => o !== y && Math.abs(o - y) < dim * 0.15)
      if (!bar && y > dim * 0.2 && y < dim * 0.8) out.push({ axis, at: y })
    }
  }
  return out
}

const near = (a: Split, b: Split) => a.axis === b.axis && Math.abs(a.at - b.at) <= 6
const big = (fr: NonNullable<Faces>) => fr.faces.filter((f) => f[3] >= fr.h * FACE_MIN)

/** what cuts a face in a frame: an edge of the frame, or a split screen's seam */
function cutBy(fr: NonNullable<Faces>, splits: Split[]): string | null {
  for (const [x, y, fw, fh] of big(fr)) {
    if (x <= 1) return "the left edge"
    if (x + fw >= fr.w - 1) return "the right edge"
    if (y <= 1) return "the top edge"
    if (y + fh >= fr.h - 1) return "the bottom edge"
    for (const sp of splits) {
      const [a, b] = sp.axis === "h" ? [y, y + fh] : [x, x + fw]
      if ((a < sp.at - 2 && b > sp.at + 2) || Math.abs(b - sp.at) <= 3 || Math.abs(a - sp.at) <= 3) return "the split screen's seam"
    }
  }
  return null
}

/** runs of the same thing in consecutive frames, as [from, to) in frames */
function runs<T>(xs: (T | null)[], same: (a: T, b: T) => boolean = (a, b) => a === b): [number, number, T][] {
  const out: [number, number, T][] = []
  for (let i = 0; i < xs.length; ) {
    const x = xs[i]
    if (x === null) {
      i++
      continue
    }
    let j = i + 1
    while (j < xs.length && xs[j] !== null && same(x, xs[j] as T)) j++
    out.push([i, j, x])
    i = j
  }
  return out
}

/** faces cut by the frame's edges or a split screen's seam (a second or more an error, shorter a warning), and split
 *  screens that hold one person (a split is for two things at once) */
export function cutFaces(frames: Faces[], rate = RATE): Finding[] {
  const raw = frames.map((fr) => (fr ? splitsOf(fr) : []))
  // a seam held for a second or more: a split screen; a line for a frame is a cut or a flash
  const held = raw.map((ss, i) => ss.filter((s) => (raw[i - 1] ?? []).some((o) => near(s, o)) || (raw[i + 1] ?? []).some((o) => near(s, o))))
  const out: Finding[] = []
  const hits = frames.map((fr, i) => (fr ? cutBy(fr, held[i]) : null))
  for (const [i, j, by] of runs(hits)) {
    const from = i / rate
    const to = j / rate
    out.push({ level: to - from >= 1 ? "error" : "warn", kind: "face-cut", at: from, until: to, says: `a face is cut by ${by} from ${from.toFixed(1)} s to ${to.toFixed(1)} s: reframe on the person (both eyes, mouth and chin inside the picture)${by.includes("seam") ? ", or cut between two singles" : " or cut away"}` })
  }
  const split = held.map((ss) => ss[0] ?? null)
  for (const [i, j, sp] of runs(split, near)) {
    if (j - i < 2) continue
    // whose face is on each side of the seam, frame by frame
    let one = 0
    let any = 0
    for (let k = i; k < j; k++) {
      const fr = frames[k]
      if (!fr) continue
      const sides = new Set(big(fr).map(([x, y, fw, fh]) => ((sp.axis === "h" ? y + fh / 2 : x + fw / 2) < sp.at ? 0 : 1)))
      if (sides.size) any++
      if (sides.size === 1) one++
    }
    const from = i / rate
    const to = j / rate
    const where = sp.axis === "h" ? "top and bottom" : "side by side"
    if (any && one * 2 > any) out.push({ level: "warn", kind: "split", at: from, until: to, says: `a split screen (${where}) from ${from.toFixed(1)} s to ${to.toFixed(1)} s shows a face in only one half: split only for two things at once (two people reacting, a face and the product), else cut between singles` })
  }
  return out
}

/** texts under the size floor, from a render manifest: each text once, at its first frame */
export function smallText(manifest: any): Finding[] {
  const w = Number(manifest?.width) || 1080
  const h = Number(manifest?.height) || 1920
  const fps = Number(manifest?.fps) || 30
  const scale = 1080 / Math.min(w, h)
  const seen = new Map<string, { at: number; px: number }>()
  for (const fr of (manifest?.frames ?? []) as any[]) {
    for (const t of (fr?.text ?? []) as any[]) {
      const text = String(t?.text ?? "").trim()
      if (!text || (t?.opacity ?? 1) <= 0.05) continue
      const px = Number(t?.size) * scale
      if (!Number.isFinite(px) || px <= 0) continue
      const was = seen.get(text)
      if (!was || px > was.px) seen.set(text, { at: was?.at ?? Number(fr?.frame ?? 0) / fps, px })
    }
  }
  const out: Finding[] = []
  for (const [text, { at, px }] of seen) {
    const q = text.length > 40 ? `"${text.slice(0, 40)}..."` : `"${text}"`
    if (px < TEXT_ERROR) out.push({ level: "error", kind: "text-small", at, says: `${q} is ${px.toFixed(0)} px (in a 1080-wide frame): under ${TEXT_ERROR} px no one reads it on a phone. Make it bigger, or cut words so it fits bigger` })
    else if (px < TEXT_WARN) out.push({ level: "warn", kind: "text-small", at, says: `${q} is ${px.toFixed(0)} px: small for anything but a badge or small print (captions 52 px, hooks 64 px)` })
  }
  return out.sort((a, b) => (a.at ?? 0) - (b.at ?? 0))
}

function ffmpeg(args: string[]) {
  const r = spawnSync("ffmpeg", args, { encoding: "utf8" })
  if (r.status !== 0) throw new Error(`ffmpeg: ${(r.stderr ?? "").slice(-400)}`)
}

/** VIDEO's faces, two frames a second, by the farm */
export async function faces(file: string, f: typeof fetch = fetch): Promise<Faces[]> {
  const url = process.env.RAEA_CLOUD_GPU_URL
  const token = process.env.RAEA_CLOUD_GPU_TOKEN ?? ""
  if (!url) throw new Error("the faces check needs the farm (RAEA_CLOUD_GPU_URL); this box has none set")
  const dir = mkdtempSync(path.join(os.tmpdir(), "raea-frames-"))
  try {
    ffmpeg(["-v", "error", "-y", "-i", file, "-vf", `fps=${RATE},scale='if(gt(iw,ih),-2,${SIDE})':'if(gt(iw,ih),${SIDE},-2)'`, "-q:v", "4", path.join(dir, "f%05d.jpg")])
    const jpgs = readdirSync(dir).filter((n) => n.endsWith(".jpg")).sort()
    const out: Faces[] = []
    for (let i = 0; i < jpgs.length; i += 300) {
      const body = JSON.stringify({ frames: jpgs.slice(i, i + 300).map((n) => readFileSync(path.join(dir, n)).toString("base64")) })
      let r: Response | undefined
      let why = ""
      for (let k = 0; k < 4; k++) {
        r = await f(`${url.replace(/\/$/, "")}/v1/faces`, { method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body, signal: AbortSignal.timeout(300_000) })
        if (r.ok) break
        why = `${r.status} ${(await r.text()).slice(0, 300)}`
        if (r.status < 500) break
        await new Promise((ok) => setTimeout(ok, 1000 * 2 ** k))
      }
      if (!r?.ok) throw new Error(`the farm couldn't look at ${path.basename(file)}: ${why}`)
      out.push(...((await r.json()) as { frames: Faces[] }).frames)
    }
    return out
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

function flag(args: string[], name: string): string | undefined {
  const i = args.indexOf(name)
  return i >= 0 ? args[i + 1] : undefined
}

/** the manifest a render of VIDEO wrote: --manifest, or VIDEO.render.json beside it */
export const manifestOf = (video: string) => `${video.slice(0, video.length - path.extname(video).length)}.render.json`

export async function main(args: string[]): Promise<number> {
  const a = args[0] === "check" ? args.slice(1) : args
  const video = a.find((x, i) => !x.startsWith("-") && !["--manifest", "-o"].includes(a[i - 1] ?? ""))
  if (!video || !existsSync(video)) {
    console.error("usage: raea frames check VIDEO [--manifest RENDER.json] [-o REPORT.json]   (faces cut by the frame, and text too small to read)")
    return 2
  }
  const m = flag(a, "--manifest") ?? manifestOf(video)
  const found: Finding[] = []
  let looked = 0
  try {
    const fr = await faces(video)
    looked = fr.length
    found.push(...cutFaces(fr))
  } catch (e) {
    console.log(`WARN  the faces check couldn't run: ${(e as Error).message}`)
  }
  const texts = existsSync(m) ? smallText(JSON.parse(readFileSync(m, "utf8"))) : null
  if (texts) found.push(...texts)
  const despite = `${video.slice(0, video.length - path.extname(video).length)}.despite.txt`
  const why = existsSync(despite) ? readFileSync(despite, "utf8").trim() : ""
  const out = found.map((x) => (why && x.kind === "face-cut" ? { ...x, level: "warn" as const, says: `${x.says} (despite: ${why.slice(0, 200)})` } : x)).sort((p, q) => (p.at ?? 0) - (q.at ?? 0))
  for (const x of out) console.log(`${x.level === "error" ? "ERROR" : "WARN "} ${x.at === null ? "" : `${x.at.toFixed(1)} s  `}${x.says}`)
  const errors = out.filter((x) => x.level === "error").length
  const report = flag(a, "-o") ?? `${video.slice(0, video.length - path.extname(video).length)}.frames-check.json`
  writeFileSync(report, JSON.stringify({ video, looked_at: looked, text_checked: texts !== null, findings: out }, null, 1))
  console.log(`${errors ? "FAILED" : "PASSED"}  ${path.basename(video)}: ${looked} frames looked at for faces, ${texts === null ? "no manifest for its text" : `${new Set(texts.map((t) => t.says)).size} small texts`}, ${errors} errors, ${out.length - errors} warnings; ${path.basename(report)}`)
  return errors ? 1 : 0
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const a = process.argv.slice(2)
  main(a[0] === "frames" ? a.slice(1) : a).then(
    (c) => process.exit(c),
    (e) => (console.error(`raea frames: ${(e as Error).message}`), process.exit(1)),
  )
}

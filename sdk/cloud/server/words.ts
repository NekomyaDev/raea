// `raea words` on a raea cloud box: what a take or an export says, word by word, and whether an export says what its
// cut meant it to. raea's own transcriber is parakeet-mlx (Mac only), so a box had none: agents guessed `raea
// transcribe`, cut on guessed frames and shipped words clipped by a cut or faded under a crossfade (11-25, Shane: "cuts
// where it applies crossfades on audio that make it so you can't hear it, and cuts where it will cut early over
// someone speaking ... before exporting the audio requires a transcription pass to be sure it matches").
//
//   raea words FILE [-o OUT.words.json] [--language en] [--fades]
//       FILE's sound heard by Whisper large-v3 (the farm's POST /v1/words, the model raea cloud's library index uses)
//       into FILE's words.json beside it ([{text, start_ms, end_ms}], what `raea cut`, captions and --words read)
//   raea words check VIDEO --expect LINES.txt [-o REPORT.json] [--language en]
//       VIDEO heard, then held to the lines its cut should say (plain text, in order): a word missing or cut short is
//       an error; a word heard differently, speech fading under a crossfade, or a word running into the first or last
//       frame is a warning. Exit 1 on any error. The VIDEO's words.json is kept for its captions.
//
// The sound goes as 16 kHz mono MP3 in 10-minute pieces (`raea media ffmpeg`), to RAEA_CLOUD_GPU_URL with its token.
import { spawnSync } from "node:child_process"
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { pathToFileURL } from "node:url"

const REAL = process.env.RAEA_REAL ?? "/opt/raea/bin/raea"
const PIECE_S = 600

export type Word = { text: string; start: number; end: number }
export type Finding = { level: "error" | "warn" | "note"; kind: "missing" | "clipped" | "changed" | "faded" | "edge" | "extra"; at: number | null; says: string }

/** a word as it's compared: lower case, letters and digits only ("$1,000." and "1000" are the same word) */
export const norm = (w: string) => w.toLowerCase().normalize("NFKD").replace(/[^\p{L}\p{N}]+/gu, "")

/** the lines' words and the heard words aligned (longest common subsequence), as pairs: [expected index | -1, heard index | -1] */
export function align(want: string[], heard: string[]): [number, number][] {
  const n = want.length
  const m = heard.length
  const L = Array.from({ length: n + 1 }, () => new Uint16Array(m + 1))
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) L[i][j] = want[i] === heard[j] ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1])
  const out: [number, number][] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (want[i] === heard[j]) out.push([i++, j++])
    // a word for a word costs the alignment nothing: kept side by side, so a word heard cut short reads as one
    else if (L[i + 1][j + 1] === L[i][j]) out.push([i++, -1], [-1, j++])
    else if (L[i + 1][j] >= L[i][j + 1]) out.push([i++, -1])
    else out.push([-1, j++])
  }
  while (i < n) out.push([i++, -1])
  while (j < m) out.push([-1, j++])
  return out
}

/** one heard word is the other cut short: "test" for "tests", "abl" for "able" (three letters at least) */
const cutShort = (want: string, got: string) => got.length >= 2 && got.length < want.length && (want.startsWith(got) || want.endsWith(got))

/** the expected lines against what was heard */
export function compare(lines: string, heard: Word[]): Finding[] {
  const wantRaw = lines.split(/\s+/).filter((w) => norm(w))
  const want = wantRaw.map(norm)
  const got = heard.map((w) => norm(w.text))
  const pairs = align(want, got)
  const out: Finding[] = []
  // a missing word next to an extra one is one word heard differently (or cut short): pair them up
  for (let k = 0; k < pairs.length; k++) {
    const [i, j] = pairs[k]
    if (i >= 0 && j >= 0) continue
    const next = pairs[k + 1]
    if (i >= 0 && next && next[0] < 0 && next[1] >= 0) {
      const h = heard[next[1]]
      out.push(cutShort(want[i], got[next[1]]) ? { level: "error", kind: "clipped", at: h.start, says: `"${wantRaw[i]}" is cut short: heard "${h.text}" at ${h.start.toFixed(2)} s. Move the cut into the gap after the word` } : { level: "warn", kind: "changed", at: h.start, says: `"${wantRaw[i]}" heard as "${h.text}" at ${h.start.toFixed(2)} s: listen there; if the word is clipped or muddied, fix the cut` })
      k++
      continue
    }
    if (i < 0 && next && next[0] >= 0 && next[1] < 0) {
      const h = heard[j]
      out.push(cutShort(want[next[0]], got[j]) ? { level: "error", kind: "clipped", at: h.start, says: `"${wantRaw[next[0]]}" is cut short: heard "${h.text}" at ${h.start.toFixed(2)} s. Move the cut into the gap after the word` } : { level: "warn", kind: "changed", at: h.start, says: `"${wantRaw[next[0]]}" heard as "${h.text}" at ${h.start.toFixed(2)} s: listen there; if the word is clipped or muddied, fix the cut` })
      k++
      continue
    }
    if (i >= 0) {
      // where it should have been: after the last word heard before it
      const before = pairs.slice(0, k).reverse().find((p) => p[1] >= 0)
      const at = before ? heard[before[1]].end : 0
      out.push({ level: "error", kind: "missing", at, says: `"${wantRaw[i]}" isn't heard (expected near ${at.toFixed(2)} s): a cut, a fade or the mix took it` })
    } else out.push({ level: "note", kind: "extra", at: heard[j].start, says: `"${heard[j].text}" at ${heard[j].start.toFixed(2)} s isn't in the lines` })
  }
  return out
}

/** the level every 10 ms, in dB, of 16 kHz mono 16-bit samples */
export function levels(pcm: Int16Array): number[] {
  const hop = 160
  const out: number[] = []
  for (let s = 0; s + hop <= pcm.length; s += hop) {
    let e = 0
    for (let k = s; k < s + hop; k++) e += (pcm[k] / 32768) ** 2
    out.push(10 * Math.log10(e / hop + 1e-10))
  }
  return out
}

/** speech that fades under a crossfade: inside the heard words, a run of 120 ms or more at least 14 dB under the speech's
 *  own median, that comes back up within the same word or the next (a fade out and in across a cut). And a word running
 *  into the first or last 40 ms of the piece, which is speech cut by the piece's own edge */
export function fades(db: number[], heard: Word[], seconds: number): Finding[] {
  const inside: number[] = []
  for (const w of heard) for (let f = Math.floor(w.start * 100); f < Math.min(db.length, Math.ceil(w.end * 100)); f++) inside.push(db[f])
  if (inside.length < 20) return []
  const median = [...inside].sort((a, b) => a - b)[Math.floor(inside.length / 2)]
  const out: Finding[] = []
  const quiet = (f: number) => db[f] < median - 14
  let lastAt = -9
  for (let n = 0; n < heard.length; n++) {
    const w = heard[n]
    const a = Math.floor(w.start * 100)
    const b = Math.min(db.length, Math.ceil(w.end * 100))
    let run = 0
    for (let f = a; f < b; f++) {
      run = quiet(f) ? run + 1 : 0
      if (run === 12 && f / 100 - lastAt > 1) {
        lastAt = f / 100
        out.push({ level: "warn", kind: "faded", at: (f - 11) / 100, says: `"${w.text}" at ${w.start.toFixed(2)} s dips ${(median - db[f]).toFixed(0)} dB under the speech for ${run * 10}+ ms: a crossfade or fade over speech? Dialogue joins are hard cuts in the gap between words, with fades of 20 ms at most` })
      }
    }
  }
  const first = heard[0]
  const last = heard[heard.length - 1]
  // Whisper puts a first word at 0 whether or not it's cut: only a sound already at speech level on the first (or last)
  // 30 ms is speech the edge cut into
  const loud = (fs: number[]) => fs.length > 0 && fs.every((f) => db[f] !== undefined && db[f] > median - 8)
  const n0 = db.length
  if (first && first.start < 0.04 && loud([0, 1, 2])) out.push({ level: "warn", kind: "edge", at: 0, says: `the first word "${first.text}" starts on the first frame: is its start cut off?` })
  if (last && seconds - last.end < 0.04 && loud([n0 - 3, n0 - 2, n0 - 1])) out.push({ level: "warn", kind: "edge", at: last.end, says: `the last word "${last.text}" runs to the last frame: is its end cut off?` })
  return out
}

function ffmpeg(args: string[]) {
  const r = spawnSync(REAL, ["media", "ffmpeg", ...args], { encoding: "utf8" })
  if (r.status !== 0) {
    const r2 = spawnSync("ffmpeg", args, { encoding: "utf8" })
    if (r2.status !== 0) throw new Error(`ffmpeg ${args.join(" ")}: ${(r.stderr || r2.stderr || "").slice(-400)}`)
  }
}

function seconds(file: string): number {
  const r = spawnSync(REAL, ["media", "ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file], { encoding: "utf8" })
  const s = Number.parseFloat(r.stdout)
  if (Number.isFinite(s)) return s
  const r2 = spawnSync("ffprobe", ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file], { encoding: "utf8" })
  const s2 = Number.parseFloat(r2.stdout)
  if (!Number.isFinite(s2)) throw new Error(`${file}: no length (is it a video or sound file?)`)
  return s2
}

/** what FILE says, by the farm's Whisper */
export async function hear(file: string, language?: string, f: typeof fetch = fetch): Promise<Word[]> {
  const url = process.env.RAEA_CLOUD_GPU_URL
  const token = process.env.RAEA_CLOUD_GPU_TOKEN ?? ""
  if (!url) throw new Error("raea words needs the farm (RAEA_CLOUD_GPU_URL), which hears it with Whisper; this box has none set")
  const len = seconds(file)
  const dir = mkdtempSync(path.join(os.tmpdir(), "raea-words-"))
  try {
    const out: Word[] = []
    for (let at = 0; at < len; at += PIECE_S) {
      const mp3 = path.join(dir, `p${at}.mp3`)
      ffmpeg(["-v", "error", "-y", "-ss", String(at), "-t", String(PIECE_S), "-i", file, "-vn", "-ac", "1", "-ar", "16000", "-b:a", "32k", mp3])
      if (!existsSync(mp3)) throw new Error(`${file} has no sound to hear`)
      let r: Response | undefined
      let why = ""
      for (let k = 0; k < 4; k++) {
        r = await f(`${url.replace(/\/$/, "")}/v1/words${language ? `?language=${encodeURIComponent(language)}` : ""}`, { method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "audio/mpeg" }, body: readFileSync(mp3), signal: AbortSignal.timeout(330_000) })
        if (r.ok) break
        why = `${r.status} ${(await r.text()).slice(0, 300)}`
        if (r.status < 500) break
        await new Promise((ok) => setTimeout(ok, 1000 * 2 ** k))
      }
      if (!r?.ok) throw new Error(`the farm couldn't hear ${path.basename(file)}: ${why}`)
      const b = (await r.json()) as { words: Word[] }
      out.push(...split(b.words).map((w) => ({ text: w.text, start: at + w.start, end: at + w.end })))
    }
    return out
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

/** Whisper sometimes hands back two words as one ("for $60"): one word each, sharing its time by their lengths */
export function split(ws: Word[]): Word[] {
  return ws.flatMap((w) => {
    const parts = w.text.split(/\s+/).filter(Boolean)
    if (parts.length < 2) return [w]
    const total = parts.reduce((n, p) => n + p.length, 0)
    let t = w.start
    return parts.map((p) => {
      const d = ((w.end - w.start) * p.length) / total
      const x = { text: p, start: t, end: t + d }
      t += d
      return x
    })
  })
}

const wordsPath = (file: string) => `${file.slice(0, file.length - path.extname(file).length)}.words.json`
const asRaea = (ws: Word[]) => ws.map((w) => ({ text: w.text, start_ms: Math.round(w.start * 1000), end_ms: Math.round(w.end * 1000) }))

function flag(args: string[], name: string): string | undefined {
  const i = args.indexOf(name)
  return i >= 0 ? args[i + 1] : undefined
}

export async function main(args: string[]): Promise<number> {
  const language = flag(args, "--language")
  if (args[0] === "check") {
    const video = args[1]
    const expect = flag(args, "--expect")
    if (!video || !expect || !existsSync(video) || !existsSync(expect)) {
      console.error("usage: raea words check VIDEO --expect LINES.txt [-o REPORT.json] [--language en]   (LINES.txt: the words the cut should say, in order)")
      return 2
    }
    const heard = await hear(video, language)
    writeFileSync(wordsPath(video), `${JSON.stringify(asRaea(heard))}\n`)
    const findings = [...compare(readFileSync(expect, "utf8"), heard), ...soundFindings(video, heard)].sort((a, b) => (a.at ?? 0) - (b.at ?? 0))
    const errors = findings.filter((x) => x.level === "error").length
    const warns = findings.filter((x) => x.level === "warn").length
    const report = { object: "words.check", video, expect, passed: errors === 0, errors, warnings: warns, heard: heard.map((w) => w.text).join(" "), findings }
    const o = flag(args, "-o") ?? `${video.slice(0, video.length - path.extname(video).length)}.words-check.json`
    writeFileSync(o, `${JSON.stringify(report, null, 2)}\n`)
    for (const x of findings.filter((x) => x.level !== "note")) console.log(`${x.level === "error" ? "ERROR" : "WARN "} ${x.at === null ? "" : `${x.at.toFixed(2)} s  `}${x.says}`)
    const extra = findings.filter((x) => x.level === "note").length
    console.log(`${errors ? "FAILED" : "PASSED"}  ${path.basename(video)}: ${heard.length} words heard, ${errors} errors, ${warns} warnings${extra ? `, ${extra} words not in the lines` : ""}; ${o}`)
    return errors ? 1 : 0
  }
  const file = args.find((a, i) => !a.startsWith("-") && args[i - 1] !== "-o" && args[i - 1] !== "--language")
  if (!file || !existsSync(file)) {
    console.error("usage: raea words FILE [-o OUT.words.json] [--language en]   |   raea words check VIDEO --expect LINES.txt [-o REPORT.json]")
    return 2
  }
  const heard = await hear(file, language)
  const o = flag(args, "-o") ?? wordsPath(file)
  writeFileSync(o, `${JSON.stringify(asRaea(heard))}\n`)
  console.log(`${o}: ${heard.length} words, ${heard.length ? heard[heard.length - 1].end.toFixed(1) : 0} s`)
  console.log(heard.map((w) => w.text).join(" "))
  if (args.includes("--fades")) for (const x of soundFindings(file, heard)) console.log(`WARN  ${x.at === null ? "" : `${x.at.toFixed(2)} s  `}${x.says}`)
  return 0
}

/** fades and edge words in FILE's sound against what was heard */
function soundFindings(file: string, heard: Word[]): Finding[] {
  const dir = mkdtempSync(path.join(os.tmpdir(), "raea-words-"))
  try {
    const raw = path.join(dir, "a.raw")
    ffmpeg(["-v", "error", "-y", "-i", file, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", "-f", "s16le", raw])
    const b = readFileSync(raw)
    return fades(levels(new Int16Array(b.buffer, b.byteOffset, Math.floor(b.length / 2))), heard, seconds(file))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

/** after a video render (offload.ts): its words heard and kept beside it, held to OUT.lines.txt when there is one. The
 *  exit code is the check's when it ran (1 on an error), else 0; a farm that can't hear it never fails the render */
export async function afterRender(out: string, say: (s: string) => void): Promise<number> {
  const lines = `${out.slice(0, out.length - path.extname(out).length)}.lines.txt`
  try {
    if (existsSync(lines)) {
      say(`the words check: ${path.basename(out)} held to ${path.basename(lines)}`)
      const code = await main(["check", out, "--expect", lines])
      if (code) say(`${path.basename(out)} doesn't say its lines: fix every ERROR above (move the cut into the gap between words, take the fade off the speech) and render again before any review`)
      return code
    }
    say(`the words heard in ${path.basename(out)} (no ${path.basename(lines)} beside it to hold them to: write the lines this cut should say there, and the next render checks them)`)
    await main([out, "--fades"])
  } catch (e) {
    say(`the words pass couldn't run: ${(e as Error).message}`)
  }
  return 0
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  // `raea transcribe` is the name agents reach for: the same thing
  const a = process.argv.slice(2)
  main(a[0] === "words" || a[0] === "transcribe" ? a.slice(1) : a).then(
    (c) => process.exit(c),
    (e) => (console.error(`raea words: ${(e as Error).message}`), process.exit(1)),
  )
}

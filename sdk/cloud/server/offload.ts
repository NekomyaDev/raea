// raea on a CPU box with its renders on a GPU worker (docs/cloud.feature CL15). image/raea.sh runs this for
// `raea render ...` when RAEA_CLOUD_GPU_URL names a raea cloud server on a GPU (RAEA_CLOUD_GPU_TOKEN its token):
//
//   1. the render's project goes there as tar.zst: the folder of its composition, or the nearest one above it with
//      package.json, raea.lock or brief.ts (RAEA_CLOUD_GPU_ROOT names another), without .git or what's in out/
//      (RAEA_CLOUD_GPU_EXCLUDE: tar patterns, space-separated, to send instead);
//   2. `raea render` runs there with the same arguments, its paths made relative to that folder, and prints here as
//      it comes;
//   3. the files it made or changed land back here: an output path outside the folder (-o /tmp/x.mp4) is written
//      there under .raea-cloud-out and moved to its place after.
// In a raea repo, with this box signed in, the project's media goes by hash instead (media.ts, CR23): the farm fetches
// it from R2, and the media the render makes goes to R2 from the farm, here only as pointer files.
//
// A render that reads a path outside its project (an input, --words, ...) can't go: it runs here, on the CPU, and
// says why. So does every render when the worker can't be reached and RAEA_CLOUD_GPU_FALLBACK=cpu; otherwise that
// fails, so a box meant to render on a GPU never quietly renders on its CPU.

import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { pathToFileURL } from "node:url"
import { RaeaCloud } from "../src/index.ts"
import { excluded, excludes as mediaExcludes, farmBlobs, farmInputs, inputs, lfsOf, outputs } from "./media.ts"

const REAL = process.env.RAEA_REAL ?? "/opt/raea/bin/raea"
// flags whose value is a path raea writes; the rest of a render's path arguments are read
const WRITES = new Set(["-o", "--out", "--audio", "--manifest", "--review"])
const READS = new Set(["--input", "--words", "--perception", "--bundle", "--dir", "--taste"])
const COMPOSITION = /\.(tsx|ts|jsx|js|mjs|html)$/

const say = (s: string) => process.stderr.write(`raea cloud: ${s}\n`)

function local(args: string[], why?: string): never {
  if (why) say(`${why}, so this runs here, on the CPU`)
  const r = spawnSync(REAL, args, { stdio: "inherit" })
  process.exit(r.status ?? 1)
}

const inside = (root: string, p: string) => p === root || p.startsWith(root + path.sep)

/** The project a render's composition is in: its folder, or the nearest above it that holds a project's files. */
export function projectRoot(composition: string, cwd: string, stop = os.homedir()): string {
  const start = path.dirname(path.resolve(cwd, composition))
  for (let d = start; ; d = path.dirname(d)) {
    if (["package.json", "raea.lock", "brief.ts"].some((f) => existsSync(path.join(d, f)))) return d
    if (d === stop || d === path.dirname(d)) return start
  }
}

export interface Plan {
  root: string
  args: string[]
  /** outputs outside the root: where the worker writes them (relative to root), and where they go after */
  moves: { from: string; to: string }[]
}

/** The render's arguments made relative to its project, or why it can't leave this machine. */
export function plan(args: string[], cwd: string, root?: string): Plan | string {
  const comp = args.find((a, i) => i > 0 && !a.startsWith("-") && COMPOSITION.test(a) && !WRITES.has(args[i - 1]))
  const bundle = args[args.indexOf("--bundle") + 1]
  const from = comp ?? (args.includes("--bundle") ? bundle : undefined)
  if (!from) return "this render names no composition"
  const r = root ? path.resolve(cwd, root) : projectRoot(from, cwd)
  const moves: Plan["moves"] = []
  const out: string[] = []
  for (let i = 0; i < args.length; i++) {
    const a = args[i]
    const flag = args[i - 1]
    const isPath = i > 0 && !a.startsWith("-") && (WRITES.has(flag) || READS.has(flag) || a === from || existsSync(path.resolve(cwd, a)))
    if (!isPath) {
      out.push(a)
      continue
    }
    const abs = path.resolve(cwd, a)
    if (inside(r, abs)) out.push(path.relative(r, abs) || ".")
    else if (WRITES.has(flag)) {
      const rel = path.join(".raea-cloud-out", `${moves.length}-${path.basename(abs)}`)
      moves.push({ from: rel, to: abs })
      out.push(rel)
    } else return `${a} is outside the project sent to the GPU (${r})`
  }
  return { root: r, args: out, moves }
}

async function main() {
  const args = process.argv.slice(2)
  const url = process.env.RAEA_CLOUD_GPU_URL
  const token = process.env.RAEA_CLOUD_GPU_TOKEN ?? ""
  if (!url) local(args)
  const p = plan(args, process.cwd(), process.env.RAEA_CLOUD_GPU_ROOT)
  if (typeof p === "string") local(args, p)

  const tmp = mkdtempSync(path.join(os.tmpdir(), "raea-offload-"))
  try {
    // kits/ (raea-game-kit) is a link into the box's shared cache, hundreds of MB, that the farm can't follow: a
    // composition uses copies of the kit files it needs in its own folder (11-21: the dangling link hung farm jobs)
    const excludes = (process.env.RAEA_CLOUD_GPU_EXCLUDE ?? "./.git ./out/* ./.raea-cloud-out ./kits").split(/\s+/).filter(Boolean)
    // zstd with GNU tar (the image's); gzip with another tar (a Mac's), whose zstd needs a program of its own
    const gnu = spawnSync("tar", ["--version"], { encoding: "utf8" }).stdout?.includes("GNU")
    const sent = path.join(tmp, "in.tar")
    // the media that goes by hash: out of the tar, named in its manifest
    const lfs = lfsOf(p.root)
    let byHash = typeof lfs === "string" ? [] : await inputs(p.root, lfs, excluded(excludes))
    // not in raea's LFS: the media goes into the farm's own cache by hash, once, instead of in every render's tar
    // (RAEA_CLOUD_GPU_BLOBS=0 puts it back in the tar). A farm without the cache, or one that can't be reached here,
    // leaves it in the tar, and the health check below says why the farm can't be reached
    let viaFarm: { media: typeof byHash; sent: number } | null = null
    if (typeof lfs === "string" && process.env.RAEA_CLOUD_GPU_BLOBS !== "0") {
      viaFarm = await farmInputs(p.root, farmBlobs(url, token), excluded(excludes), say).catch((e) => (say(`the farm's media cache: ${(e as Error).message}; the media goes in the tar`), null))
      if (viaFarm) byHash = viaFarm.media
    }
    const extra: string[] = []
    if (byHash.length) {
      writeFileSync(path.join(tmp, "media-excludes"), mediaExcludes(byHash))
      writeFileSync(path.join(tmp, ".raea-cloud-media.json"), JSON.stringify({ objects: byHash }))
      extra.push("-X", path.join(tmp, "media-excludes"))
    }
    const pack = spawnSync("tar", [gnu ? "--zstd" : "-z", "-cf", sent, ...excludes.map((e) => `--exclude=${e}`), ...extra, "-C", p.root, ".", ...(byHash.length ? ["-C", tmp, ".raea-cloud-media.json"] : [])], { encoding: "utf8" })
    if (pack.status !== 0) throw new Error(`the project didn't pack: ${pack.stderr.trim()}`)
    const folder = new Uint8Array(readFileSync(sent))

    const cloud = new RaeaCloud({ url, token })
    const t0 = performance.now()
    let h
    try {
      h = await cloud.health()
    } catch (e) {
      if (process.env.RAEA_CLOUD_GPU_FALLBACK === "cpu") local(args, `the GPU worker at ${url} didn't answer (${(e as Error).message})`)
      throw new Error(`the GPU worker at ${url} didn't answer: ${(e as Error).message} (RAEA_CLOUD_GPU_FALLBACK=cpu renders here instead)`)
    }
    say(`rendering on ${new URL(url).host}, ${h.device.renders === "gpu" ? `GPU ${h.device.name}` : `its CPU (${h.device.name})`}: ${p.root} (${(folder.length / 1e6).toFixed(1)} MB) sent`)
    if (viaFarm && byHash.length) say(`${byHash.length} media files (${(byHash.reduce((n, m) => n + m.size, 0) / 1e6).toFixed(1)} MB) go by hash from the farm's cache${viaFarm.sent ? `, ${(viaFarm.sent / 1e6).toFixed(1)} MB of it sent now` : ""}`)
    else if (byHash.length) say(`${byHash.length} media files (${(byHash.reduce((n, m) => n + m.size, 0) / 1e6).toFixed(1)} MB) go by hash: the farm fetches them from R2`)
    else if (typeof lfs === "string" && existsSync(path.join(p.root, ".lfsconfig"))) say(`media goes in the tar: ${lfs}`)
    // a render farm's choices (deploy/modal/farm.py): RAEA_CLOUD_GPUS, RAEA_CLOUD_GPU_KIND, RAEA_CLOUD_MAX_USD
    const n = (v?: string) => (v ? Number(v) : undefined)
    const farm = { gpus: n(process.env.RAEA_CLOUD_GPUS), gpu: process.env.RAEA_CLOUD_GPU_KIND, maxUsd: n(process.env.RAEA_CLOUD_MAX_USD), media: typeof lfs !== "string" }
    let finish: (() => Promise<number>) | undefined
    const media = typeof lfs === "string" ? undefined : async (made: Parameters<typeof outputs>[2]) => {
      const o = await outputs(p.root, lfs, made)
      finish = o.done
      return o.send
    }
    // what the job said, kept to spot a file it wanted that stayed here (out/ isn't sent: it's renders)
    let said = ""
    const { exit, files } = await cloud.job(p.args, folder, { as: gnu ? "zst" : "gz", farm, media, onOutput: (t, s) => ((said = (said + t).slice(-20000)), (s === "out" ? process.stdout : process.stderr).write(t)) })

    const back = path.join(tmp, "out.tar")
    writeFileSync(back, files)
    const unpack = spawnSync("tar", ["-xf", back, "-C", p.root], { encoding: "utf8" })
    if (unpack.status !== 0) throw new Error(`the results didn't unpack: ${unpack.stderr.trim()}`)
    for (const m of p.moves) {
      const from = path.join(p.root, m.from)
      if (!existsSync(from)) continue
      mkdirSync(path.dirname(m.to), { recursive: true })
      // a move across filesystems (into /tmp) is a copy
      try {
        renameSync(from, m.to)
      } catch {
        spawnSync("cp", ["-R", from, m.to])
      }
    }
    rmSync(path.join(p.root, ".raea-cloud-out"), { recursive: true, force: true })
    if (finish) {
      const bytes = await finish()
      say(`its media (${(bytes / 1e6).toFixed(1)} MB) went from the farm to R2; here it's pointers (git lfs pull PATH brings one here)`)
    }
    say(`done on the GPU worker in ${((performance.now() - t0) / 1000).toFixed(1)} s, exit ${exit}`)
    // 11-25: an agent kept its words in out/words/ and four renders failed on "No such file" with no reason given
    const missed = exit ? /(?:^|[\s/])(out\/[^\s:]+): No such file/m.exec(said) : null
    if (missed && excludes.includes("./out/*")) say(`${missed[1]} wasn't sent: out/ stays here, as it holds renders. Keep what a composition reads (words, data, stills it uses) in media/ or beside the composition`)
    process.exitCode = exit
    // a video's words, heard as soon as it's back (words.ts): the cut is held to OUT.lines.txt when it has one, so a
    // word clipped by a cut or faded under a crossfade is caught before anyone reviews it (11-25, Shane)
    // the -o asked for (an output outside the project came back there by its move)
    const oi = args.indexOf("-o")
    const made = oi >= 0 ? path.resolve(process.cwd(), args[oi + 1] ?? "") : ""
    if (!exit && /\.(mp4|mov|m4v|webm)$/i.test(made) && existsSync(made) && process.env.RAEA_CLOUD_WORDS !== "0") {
      const { afterRender } = await import("./words.ts")
      const code = await afterRender(made, say)
      if (code) process.exitCode = code
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true })
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  main().catch((e) => {
    say((e as Error).message)
    process.exit(1)
  })

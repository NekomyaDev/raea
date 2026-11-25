// A box's media by hash into the farm's own cache (server/media.ts farmInputs, 11-25), against a local stand-in for
// the farm's POST /v1/blobs/have and PUT /v1/blobs/OID: node test/farm-blobs.ts from sdk/cloud.
// Checked: a project's footage and large files go by hash and its code and small files stay in the tar; the first
// render sends each once (streamed, checked against its hash), the next sends nothing and hashes nothing again; a
// file changed since is sent again; out/ and links are left alone; a farm without the cache answers null.

import { createHash } from "node:crypto"
import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs"
import { createServer } from "node:http"
import type { AddressInfo } from "node:net"
import { tmpdir } from "node:os"
import path from "node:path"
import { excluded, farmBlobs, farmInputs } from "../server/media.ts"

const ok = (m: string) => console.log(`ok ${m}`)
const assert = (c: unknown, m: string) => {
  if (!c) throw new Error(`not ok: ${m}`)
}

const held = new Map<string, number>()
let puts = 0
let cache = true
const farm = createServer((req, res) => {
  const u = new URL(req.url!, "http://x")
  if (req.headers.authorization !== "Bearer farm-token") return void res.writeHead(401).end()
  if (!cache) return void res.writeHead(404).end()
  if (req.method === "POST" && u.pathname === "/v1/blobs/have") {
    let b = ""
    req.on("data", (c) => (b += c)).on("end", () => {
      const objects = (JSON.parse(b).objects ?? []) as { oid: string; size: number }[]
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ have: objects.filter((o) => held.get(o.oid) === o.size).map((o) => o.oid) }))
    })
    return
  }
  const m = /^\/v1\/blobs\/([0-9a-f]{64})$/.exec(u.pathname)
  if (req.method === "PUT" && m) {
    const h = createHash("sha256")
    let n = 0
    req.on("data", (c: Buffer) => (h.update(c), (n += c.length))).on("end", () => {
      if (h.digest("hex") !== m[1] || n !== Number(u.searchParams.get("size"))) return void res.writeHead(400).end("wrong hash")
      held.set(m[1], n)
      puts++
      res.writeHead(200).end("{}")
    })
    return
  }
  res.writeHead(404).end()
})
await new Promise<void>((r) => farm.listen(0, "127.0.0.1", r))
const url = `http://127.0.0.1:${(farm.address() as AddressInfo).port}`

const root = mkdtempSync(path.join(tmpdir(), "farm-blobs-"))
const store = path.join(root, "..", `${path.basename(root)}-hashes.json`)
mkdirSync(path.join(root, "media"), { recursive: true })
mkdirSync(path.join(root, "out"), { recursive: true })
mkdirSync(path.join(root, "brand"), { recursive: true })
writeFileSync(path.join(root, "media", "interview.MOV"), Buffer.alloc(5 << 20, 7))
writeFileSync(path.join(root, "media", "copy.MOV"), Buffer.alloc(5 << 20, 7)) // the same bytes: one upload
writeFileSync(path.join(root, "brand", "logo.png"), Buffer.alloc(300 << 10, 3))
writeFileSync(path.join(root, "brand", "icon.png"), Buffer.alloc(2 << 10, 3)) // small: stays in the tar
writeFileSync(path.join(root, "data.bin"), Buffer.alloc(6 << 20, 1)) // large, of no media type
writeFileSync(path.join(root, "edit.tsx"), "export default () => null\n")
writeFileSync(path.join(root, "out", "old.mp4"), Buffer.alloc(5 << 20, 9)) // an old render: the tar's excludes leave it
symlinkSync(path.join(root, "media", "interview.MOV"), path.join(root, "media", "link.MOV"))

const skip = excluded(["./.git", "./out/*", "./.raea-cloud-out", "./kits"])
const blobs = farmBlobs(url, "farm-token")
const first = await farmInputs(root, blobs, skip, () => {}, store)
assert(first, "a farm with the cache answers")
const paths = first!.media.map((m) => m.path).sort()
assert(JSON.stringify(paths) === JSON.stringify(["brand/logo.png", "data.bin", "media/copy.MOV", "media/interview.MOV"]), `what goes by hash: ${paths}`)
assert(first!.media.every((m) => !m.href), "no href: the farm's own cache")
assert(puts === 3 && first!.sent === (5 << 20) + (300 << 10) + (6 << 20), `sent once each, the same bytes once: ${puts} puts, ${first!.sent}`)
ok("footage, large files and media over 256 KB go by hash, each sent once; code, small files, out/ and links stay in the tar")

const second = await farmInputs(root, blobs, skip, () => {}, store)
assert(second!.sent === 0 && puts === 3, "the next render sends nothing")
ok("the next render sends nothing: the farm has it all")

writeFileSync(path.join(root, "brand", "logo.png"), Buffer.alloc(300 << 10, 4))
const third = await farmInputs(root, blobs, skip, () => {}, store)
assert(third!.sent === 300 << 10 && puts === 4, `a changed file is sent again: ${third!.sent}`)
ok("a file changed since is hashed and sent again")

cache = false
assert((await farmInputs(root, blobs, skip, () => {}, store)) === null, "an older farm")
ok("a farm without the cache answers null: the media goes in the tar")
farm.close()

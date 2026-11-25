# A farm job's media by hash (docs/cloud-render.feature CR23): its inputs come from R2 by their Git LFS object ids,
# through URLs raea's LFS signed for the box that sent the job, into a cache on the farm's volume keyed by hash, so a
# file is fetched once across every job and each is checked against its hash; the job's own media (a render, a
# sound) goes to R2 the same way, PUT to URLs signed for it, and the box writes only pointer files. The farm never holds
# a raea credential, only signed URLs that each reach one object.
#
#   .raea-cloud-media.json   in the job's folder, from the box: {"objects": [{"path", "oid", "size", "href", "header"}]}
#                            An object with no href is one the box sent the farm itself (PUT /v1/blobs/OID, farm.py:
#                            a project not in raea's LFS, 11-25): it is used from the cache, and only from there
#   fetch(work, cache, ...)  each object into the cache (once), then into its path in the job's folder
#   outputs(work, changed)   the job's media among what it made or changed: [{"path", "oid", "size"}]
#   upload(work, objects)    PUTs each to its signed URL: [{"path", "oid", "href", "header"}]

import hashlib
import json
import os
import shutil
import urllib.request

MANIFEST = ".raea-cloud-media.json"
# media by type, whatever the size, and anything at or over LARGE (the app's host/repo.ts MEDIA and LARGE)
MEDIA = set(
    "mp4 mov m4v webm mkv avi mxf mp3 wav aiff aif flac m4a ogg opus caf png jpg jpeg heic heif webp gif tif tiff exr hdr "
    "psd blend glb gltf fbx obj usd usdz abc ply splat spz ktx2 ttf otf woff woff2 zip tar gz zst safetensors onnx bin npz "
    "npy pdf".split()
)
LARGE = 4 * 1024 * 1024
CHUNK = 1 << 20
# Cloudflare refuses Python's own User-Agent (Python-urllib) on raea's domains with a 403, signed URL or not
AGENT = {"user-agent": "raea-farm/1"}


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(CHUNK), b""):
            h.update(b)
    return h.hexdigest()


def inside(work, rel):
    p = os.path.realpath(os.path.join(work, rel))
    return p == os.path.realpath(work) or p.startswith(os.path.realpath(work) + os.sep)


def get(href, header, dest):
    req = urllib.request.Request(href, headers={**AGENT, **(header or {})})
    with urllib.request.urlopen(req, timeout=600) as r, open(dest, "wb") as f:
        shutil.copyfileobj(r, f, CHUNK)


def probe(href, header):
    """One byte through the job's own signed URL: a cached file is used only when the account could fetch it"""
    req = urllib.request.Request(href, headers={**AGENT, "range": "bytes=0-0", **(header or {})})
    with urllib.request.urlopen(req, timeout=60) as r:
        r.read(1)


def put(href, header, src, size):
    with open(src, "rb") as f:
        # as git-lfs's basic transfer sends it
        head = {**AGENT, "content-type": "application/octet-stream", **(header or {}), "content-length": str(size)}
        req = urllib.request.Request(href, data=f, method="PUT", headers=head)
        with urllib.request.urlopen(req, timeout=1800) as r:
            r.read()


def fetch(work, cache, log=print, download=get, check=probe):
    """The job's media from R2 into its folder, through the cache: {objects, fetched, cached} (bytes)."""
    m = os.path.join(work, MANIFEST)
    if not os.path.exists(m):
        return {"objects": 0, "fetched": 0, "cached": 0}
    with open(m) as f:
        objects = json.load(f)["objects"]
    os.remove(m)
    os.makedirs(cache, exist_ok=True)
    fetched = cached = 0
    for o in objects:
        oid, path, size = o["oid"], o["path"], int(o["size"])
        if len(oid) != 64 or any(c not in "0123456789abcdef" for c in oid) or not inside(work, path):
            raise ValueError(f"media: {path!r} ({oid}) isn't a path in the job or an object id")
        blob = os.path.join(cache, oid)
        if not o.get("href"):
            # sent by the box over the farm's own token (PUT /v1/blobs), checked against its hash as it came
            if not (os.path.exists(blob) and os.path.getsize(blob) == size):
                raise ValueError(f"media: {path} ({oid[:12]}…) was never sent to the farm")
            cached += size
        elif os.path.exists(blob) and os.path.getsize(blob) == size:
            # a hash alone doesn't open the cache: the job's URL, signed for its account, has to answer
            check(o["href"], o.get("header"))
            cached += size
        else:
            tmp = f"{blob}.part-{os.getpid()}"
            download(o["href"], o.get("header"), tmp)
            got = sha256(tmp)
            if got != oid or os.path.getsize(tmp) != size:
                os.remove(tmp)
                raise ValueError(f"media: {path} came back as {got[:12]}…, not {oid[:12]}…")
            os.replace(tmp, blob)
            fetched += size
        dest = os.path.join(work, path)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        if os.path.lexists(dest):
            os.remove(dest)
        # a link into the cache, not a copy: a project's footage is gigabytes, the same across every job (11-25); every
        # container of the job mounts the volume at the same place, so the link resolves on the GPUs too
        try:
            os.symlink(blob, dest)
        except OSError:
            shutil.copyfile(blob, dest)
    log(f"raea farm: {len(objects)} media files by hash, {fetched / 1e6:.1f} MB from R2, {cached / 1e6:.1f} MB already here")
    return {"objects": len(objects), "fetched": fetched, "cached": cached}


def is_media(path, size):
    ext = path.rsplit(".", 1)[-1].lower() if "." in os.path.basename(path) else ""
    return ext in MEDIA or size >= LARGE


def outputs(work, changed):
    """The job's media among the files it made or changed (paths relative to work), each with its hash and size. Files
    meant for outside the project (.raea-cloud-out) aren't: they go back whole, to be moved where the box asked."""
    out = []
    for rel in changed:
        p = os.path.join(work, rel)
        if rel.startswith(".raea-cloud-out") or not os.path.isfile(p) or os.path.islink(p):
            continue
        size = os.path.getsize(p)
        if is_media(rel, size):
            out.append({"path": rel, "oid": sha256(p), "size": size})
    return out


def upload(work, objects, log=print, send=put):
    """Each named output PUT to its signed URL: {uploaded, bytes}. An object without an href is one R2 has already."""
    n = total = 0
    for o in objects:
        if not o.get("href"):
            continue
        if not inside(work, o["path"]):
            raise ValueError(f"media: {o['path']!r} isn't a path in the job")
        p = os.path.join(work, o["path"])
        size = os.path.getsize(p)
        send(o["href"], o.get("header"), p, size)
        n += 1
        total += size
    log(f"raea farm: {n} media files to R2, {total / 1e6:.1f} MB")
    return {"uploaded": n, "bytes": total}

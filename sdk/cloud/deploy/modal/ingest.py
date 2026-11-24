# raea cloud's ingest on Modal (docs/cloud-ingest.feature): what a synced file needs that a Worker can't do itself.
# The Worker (sync-run.ts) calls it with the file at an address it can fetch (a Composio download's s3url) and keeps
# what comes back. From sdk/cloud:
#
#   MODAL_PROFILE=raea modal deploy deploy/modal/ingest.py
#
#   POST /v1/frame {url, at?}   a video's frame as a JPEG 640 px on its long side; at is where, as a fraction of its
#                               length (0.1 when left out: past a fade from black, before the end card). IG4
#   GET  /healthz
#
# Every call but /healthz carries Authorization: Bearer RAEA_INGEST_TOKEN (Modal secret raea-ingest; the Worker's
# secret of the same name). CPU only: a frame is ffmpeg seeking in the file, a second or two.

import os
import subprocess
import tempfile

import modal

app = modal.App("raea-ingest", image=modal.Image.debian_slim(python_version="3.12").apt_install("ffmpeg").pip_install("fastapi[standard]"))
secret = modal.Secret.from_name("raea-ingest")

# the largest file fetched for a frame: a phone's minute of 4K is about 400 MB
MAX_BYTES = 2 << 30
LONG_SIDE = 640


def probe_seconds(path: str) -> float:
    r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path], capture_output=True, text=True, timeout=60)
    try:
        return max(0.0, float(r.stdout.strip()))
    except ValueError:
        return 0.0


def frame(path: str, at: float) -> bytes:
    secs = probe_seconds(path)
    t = secs * min(max(at, 0.0), 0.95) if secs else 0.0
    scale = f"scale='if(gt(iw,ih),{LONG_SIDE},-2)':'if(gt(iw,ih),-2,{LONG_SIDE})'"
    for seek in (t, 0.0):
        r = subprocess.run(["ffmpeg", "-v", "error", "-ss", f"{seek:.3f}", "-i", path, "-frames:v", "1", "-vf", scale, "-q:v", "4", "-f", "image2", "-c:v", "mjpeg", "pipe:1"], capture_output=True, timeout=120)
        if r.returncode == 0 and r.stdout:
            return r.stdout
    raise RuntimeError((r.stderr or b"no frame").decode(errors="replace")[-300:])


@app.function(cpu=2, memory=4096, secrets=[secret], timeout=900, scaledown_window=120)
@modal.concurrent(max_inputs=8)
@modal.asgi_app()
def api():
    import urllib.request

    from fastapi import FastAPI, HTTPException, Request
    from fastapi.responses import PlainTextResponse, Response

    web = FastAPI()
    token = os.environ["RAEA_INGEST_TOKEN"]

    def check(req: Request):
        if req.headers.get("authorization", "").removeprefix("Bearer ") != token:
            raise HTTPException(401, "a raea ingest token is needed (Authorization: Bearer)")

    @web.get("/healthz")
    def healthz():
        return PlainTextResponse("ok")

    @web.post("/v1/frame")
    async def make_frame(req: Request):
        check(req)
        body = await req.json()
        url, at = body.get("url"), float(body.get("at", 0.1))
        if not isinstance(url, str) or not url.startswith("https://"):
            raise HTTPException(400, "url is the file's https address")
        with tempfile.NamedTemporaryFile(suffix=".media") as f:
            got = 0
            try:
                with urllib.request.urlopen(urllib.request.Request(url, headers={"user-agent": "raea-ingest/1"}), timeout=120) as r:
                    while chunk := r.read(1 << 20):
                        got += len(chunk)
                        if got > MAX_BYTES:
                            raise HTTPException(413, "the file is over 2 GB")
                        f.write(chunk)
            except HTTPException:
                raise
            except Exception as e:
                raise HTTPException(502, f"couldn't fetch the file: {e}")
            f.flush()
            try:
                jpg = frame(f.name, at)
            except Exception as e:
                raise HTTPException(422, f"no frame in it: {e}")
        return Response(jpg, media_type="image/jpeg")

    return web

# raea cloud's ingest on Modal (docs/cloud-ingest.feature): what a synced file needs that a Worker can't do itself.
# The Worker (sync-run.ts) calls it with the file at an address it can fetch (a Composio download's s3url) and keeps
# what comes back. From sdk/cloud:
#
#   MODAL_PROFILE=raea modal deploy deploy/modal/ingest.py
#
#   indexer (its own URL, raea--raea-ingest-indexer…): raea's library indexer (tools/library-index cloud-server.ts, the
#   desktop's indexFile in the cloud indexer's image: EmbeddingGemma 2 for pictures, shots, sound and words, TransNetV2
#   cuts, Whisper base): POST /v1/index {url, name, kind} -> {fragment, posters}; POST /v1/embed {texts} -> query
#   vectors. raea cloud writes the fragments to turbopuffer in raea-api's layout (IG8). Before deploying, bundle it:
#   tools/library-index/cloud-build.sh
#
#   POST /v1/frame {url, at?}   a video's frame as a JPEG 640 px on its long side; at is where, as a fraction of its
#                               length (0.1 when left out: past a fade from black, before the end card). IG4
#   GET  /healthz
#
# Every call but /healthz carries Authorization: Bearer RAEA_INGEST_TOKEN (Modal secret raea-ingest; the Worker's
# secret of the same name). The secret's OPENROUTER_API_KEY, when set, has index jobs transcribe with Whisper large-v3
# on OpenRouter. CPU only: a frame is ffmpeg seeking in the file, a second or two.

import os
import subprocess
import tempfile

import modal

HERE = os.path.dirname(os.path.abspath(__file__))
LIB = os.path.abspath(os.path.join(HERE, "..", "..", "..", "..", "tools", "library-index"))

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

    @web.post("/v1/index-jobs")
    async def submit(req: Request):
        check(req)
        b = await req.json()
        if not isinstance(b.get("url"), str) or not b["url"].startswith("https://"):
            raise HTTPException(400, "url is the file's https address")
        if b.get("kind") not in ("video", "image", "audio", "doc"):
            raise HTTPException(400, "kind is one of video, image, audio, doc")
        call = await index_job.spawn.aio(b["url"], str(b.get("name") or "file"), b["kind"])
        return {"id": call.object_id}

    @web.get("/v1/index-jobs/{job}")
    async def collect(job: str, req: Request):
        check(req)
        try:
            out = await modal.FunctionCall.from_id(job).get.aio(timeout=0)
        except modal.exception.FunctionTimeoutError:
            return {"state": "failed", "error": "the file took over an hour"}
        except (TimeoutError, modal.exception.TimeoutError):
            return {"state": "running"}
        except Exception as e:
            return {"state": "failed", "error": str(e)[:300]}
        if out.get("error"):
            return {"state": "failed", **out}
        return {"state": "done", **out}

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


# the index jobs' GPU: an L4 by default (EmbeddingGemma 2 at fp16 on CUDA 13: loaded in 6 s, a batch in under one);
# RAEA_INDEX_GPU= (empty) at deploy for the CPU at q8
GPU = os.environ.get("RAEA_INDEX_GPU", "L4") or None

# the cloud indexer's own image (tools/library-index/Dockerfile: Node, ffmpeg, EmbeddingGemma 2's and Whisper's weights
# baked in) on CUDA 13 with cuDNN 9 (onnxruntime-node 1.30's CUDA provider links CUDA 13, though its package says cuda12), its fp16 weights for the GPU beside the q8
# ones, pdftotext for documents and the raea cloud server. EmbeddingGemma 2 (the fingerprints: pictures, shots, sound,
# words) runs on the GPU; Whisper, TransNetV2's cuts and ffmpeg on the CPU beside it
def indexer_image_for(gpu):
  return (
    # on a GPU, build/Dockerfile.gpu (cloud-build.sh: the CUDA 13 base and fp16 weights written in; Modal's builds
    # don't take a build argument in FROM, so BASE as an argument left a plain Ubuntu without CUDA)
    modal.Image.from_dockerfile(os.path.join(LIB, "build", "Dockerfile.gpu") if gpu else os.path.join(LIB, "Dockerfile"), context_dir=LIB, add_python="3.12")
    .apt_install("poppler-utils")
    .env({"RAEA_LIBRARY_DEVICE": "cuda", "RAEA_LIBRARY_DTYPE": "fp16"} if gpu else {})
    .add_local_file(os.path.join(LIB, "build", "cloud-server.mjs"), "/app/cloud-server.mjs")
    .add_local_file(os.path.join(LIB, "build", "cloud-cli.mjs"), "/app/cloud-cli.mjs")
  )


# index jobs on the GPU when there is one; the search-words server on the CPU (a query is one short text, seconds on a
# CPU, and a GPU held five minutes for it isn't worth it; q8 and fp16 vectors of the same model compare)
indexer_image = indexer_image_for(GPU)
server_image = indexer_image_for(None)


# one file indexed as a job (tools/library-index cloud-cli.ts), for a file of any length: raea cloud submits it through
# the API's POST /v1/index-jobs and collects it with GET /v1/index-jobs/ID on its next alarm (index-run.ts)
@app.function(image=indexer_image, gpu=GPU, cpu=4, memory=16384, secrets=[secret], timeout=3600, max_containers=12, scaledown_window=120)
def index_job(url: str, name: str, kind: str) -> dict:
    import json
    import subprocess as sp

    with tempfile.TemporaryDirectory() as d:
        inp, out = os.path.join(d, "in.json"), os.path.join(d, "out.json")
        with open(inp, "w") as f:
            json.dump({"url": url, "name": name, "kind": kind}, f)
        # what's said: Whisper large-v3 on OpenRouter when the secret has its key (words.ts), else Whisper base here
        words = "openrouter" if os.environ.get("OPENROUTER_API_KEY") else "whisper"
        r = sp.run(["node", "/app/cloud-cli.mjs", inp, out], cwd="/app", capture_output=True, text=True, timeout=3500, env={**os.environ, "RAEA_LIBRARY_WORDS": words})
        if r.returncode != 0 or not os.path.exists(out):
            return {"error": f"the indexer stopped ({r.returncode}): {(r.stderr or '')[-300:]}"}
        with open(out) as f:
            return json.load(f)


# a GPU only while there's work: five idle minutes and the container goes
@app.function(image=server_image, cpu=4, memory=12288, secrets=[secret], timeout=3600, scaledown_window=300)
@modal.concurrent(max_inputs=4)
@modal.web_server(8080, startup_timeout=180)
def indexer():
    import subprocess as sp

    sp.Popen(["node", "/app/cloud-server.mjs"], cwd="/app", env={**os.environ, "PORT": "8080"})


# the GPU checked: RAEA_INDEX_GPU=L4 MODAL_PROFILE=raea modal run deploy/modal/ingest.py::gpu_probe. EmbeddingGemma 2
# loaded on the GPU at fp16 and two texts embedded, each step timed
PROBE = """
const t0 = Date.now(), log = (...a) => console.log(`[${((Date.now() - t0) / 1000).toFixed(1)}s]`, ...a)
const tf = await import("@huggingface/transformers")
tf.env.cacheDir = "/models"
const id = "onnx-community/embeddinggemma-2-ONNX"
const p = await tf.AutoProcessor.from_pretrained(id)
log("model fp16 on", process.env.RAEA_LIBRARY_DEVICE)
const m = await tf.AutoModel.from_pretrained(id, { dtype: "fp16", device: process.env.RAEA_LIBRARY_DEVICE || "cpu" })
log("loaded")
for (let k = 0; k < 2; k++) { const o = await m(await p(["a white rabbit in a forest", "a quarterly sales spreadsheet"])); log("two texts", o.sentence_embedding.dims, o.sentence_embedding.tolist()[0].slice(0, 3)) }
process.exit(0)
"""


@app.function(image=indexer_image, gpu=GPU, cpu=4, memory=16384, timeout=900)
def gpu_probe() -> str:
    import subprocess as sp

    def run(cmd: str, t: int = 600) -> str:
        r = sp.run(["bash", "-c", cmd], capture_output=True, text=True, timeout=t, cwd="/app")
        return (r.stdout + r.stderr)[-3000:]

    out = run("nvidia-smi | grep -o 'CUDA Version: [0-9.]*'; nvidia-smi --query-gpu=name --format=csv,noheader")
    out += run("echo CUDA_VERSION=$CUDA_VERSION NV_CUDA=$NV_CUDA_LIB_VERSION LD=$LD_LIBRARY_PATH; ls -d /usr/local/cuda* 2>&1; find / -xdev \\( -name 'libcublas.so*' -o -name 'libcudart.so*' -o -name 'libcudnn.so*' \\) 2>/dev/null | head; cat /etc/os-release | head -2")
    out += run("ldd node_modules/onnxruntime-node/bin/napi-v*/linux/x64/libonnxruntime_providers_cuda.so | grep 'not found' || echo 'cuda provider: every library found'")
    with open("/app/probe.mjs", "w") as f:
        f.write(PROBE)
    out += run("node probe.mjs 2>&1 | cut -c1-300")
    print(out)
    return out

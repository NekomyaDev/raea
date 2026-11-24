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

    # a fingerprint's model (fingerprint_fit below): trained as a job, its state collected like an index job's
    @web.post("/v1/fingerprints/fit")
    async def fp_fit(req: Request):
        check(req)
        b = await req.json()
        if not all(isinstance(b.get(k), str) and b[k] for k in ("workspace", "fingerprint")) or not isinstance(b.get("modalities"), dict):
            raise HTTPException(400, "workspace, fingerprint and modalities {v|a|t: {good, bad, background}}")
        call = await fingerprint_fit.spawn.aio(b["workspace"], b["fingerprint"], b["modalities"])
        return {"id": call.object_id}

    @web.get("/v1/fingerprints/jobs/{job}")
    async def fp_job(job: str, req: Request):
        check(req)
        try:
            out = await modal.FunctionCall.from_id(job).get.aio(timeout=0)
        except (TimeoutError, modal.exception.TimeoutError):
            return {"state": "running"}
        except Exception as e:
            return {"state": "failed", "error": str(e)[:300]}
        return {"state": "failed", **out} if out.get("error") else {"state": "done", **out}

    @web.post("/v1/fingerprints/score")
    async def fp_score(req: Request):
        check(req)
        b = await req.json()
        if not all(isinstance(b.get(k), str) and b[k] for k in ("workspace", "fingerprint")) or not isinstance(b.get("vectors"), dict):
            raise HTTPException(400, "workspace, fingerprint and vectors {v|a|t: [f16 base64]}")
        return await fingerprint_score.remote.aio(b["workspace"], b["fingerprint"], b["vectors"])

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


# ---- a fingerprint's model (raea cloud's fingerprint-model.ts; Shane 11-24: "our embedding models for fingerprints
# stored and runnable from Modal GPU") ----
# Per modality (v picture, a sound, t words), a linear head on EmbeddingGemma 2's 768-d vectors, trained on the GPU:
# a fingerprint's good references against its bad ones, and, while it has few or no bad ones, against a sample of the
# workspace's other files (what's distinctive in it, against the rest of the library). Its held-out separation (AUC,
# k-fold) says how much it has learned. Kept on the volume raea-fingerprints at <workspace>/<fingerprint>.json, the
# newest one per fingerprint; fingerprint_score loads it on the GPU and scores vectors (a file's segments) 0..1.
# training.ts's SigLIP fine-tune replaces the head once a fingerprint has 200 judged examples.
fp_volume = modal.Volume.from_name("raea-fingerprints", create_if_missing=True)
fp_image = modal.Image.debian_slim(python_version="3.12").pip_install("torch==2.8.0", "numpy")


def _fp_path(workspace: str, fingerprint: str) -> str:
    import re

    safe = lambda x: re.sub(r"[^A-Za-z0-9._-]+", "-", x)[:80]
    return f"/fp/{safe(workspace)}/{safe(fingerprint)}.json"


def _vectors(xs):
    import base64

    import numpy as np

    rows = [np.frombuffer(base64.b64decode(x), dtype="<f2").astype("float32") for x in xs or [] if isinstance(x, str)]
    rows = [r for r in rows if r.shape == (768,)]
    return np.stack(rows) if rows else np.zeros((0, 768), dtype="float32")


def _auc(pos, neg) -> float:
    import numpy as np

    if not len(pos) or not len(neg):
        return None
    allv = np.concatenate([pos, neg])
    ranks = allv.argsort().argsort() + 1.0
    return float((ranks[: len(pos)].sum() - len(pos) * (len(pos) + 1) / 2) / (len(pos) * len(neg)))


def _r4(x):
    return None if x is None else round(x, 4)


def _train(torch, good, neg, wneg, dev, steps=400):
    """logistic regression on the GPU, classes balanced, started from the difference of their means"""
    X = torch.tensor(__import__("numpy").concatenate([good, neg]), device=dev)
    y = torch.cat([torch.ones(len(good), device=dev), torch.zeros(len(neg), device=dev)])
    wt = torch.cat([torch.full((len(good),), 0.5 / len(good), device=dev), torch.tensor(wneg, device=dev) * (0.5 / float(sum(wneg)))])
    init = X[: len(good)].mean(0) - X[len(good) :].mean(0)
    w = (init / (init.norm() + 1e-6) * 4).clone().requires_grad_(True)
    b = torch.zeros(1, device=dev, requires_grad=True)
    opt = torch.optim.Adam([w, b], lr=0.05)
    for _ in range(steps):
        opt.zero_grad()
        logit = X @ w + b
        loss = (torch.nn.functional.binary_cross_entropy_with_logits(logit, y, reduction="none") * wt).sum() + 1e-3 * (w * w).sum()
        loss.backward()
        opt.step()
    return w.detach(), b.detach()


@app.function(image=fp_image, gpu=GPU, volumes={"/fp": fp_volume}, timeout=900)
def fingerprint_fit(workspace: str, fingerprint: str, modalities: dict) -> dict:
    import json
    import time

    import numpy as np
    import torch

    dev = "cuda" if torch.cuda.is_available() else "cpu"
    out = {"workspace": workspace, "fingerprint": fingerprint, "model": "embeddinggemma-2@768", "version": int(time.time() * 1000), "device": torch.cuda.get_device_name(0) if dev == "cuda" else "cpu", "modalities": {}}
    for m, d in (modalities or {}).items():
        if m not in ("v", "a", "t"):
            continue
        good, bad, bg = _vectors(d.get("good")), _vectors(d.get("bad")), _vectors(d.get("background"))
        if len(good) < 3 or len(bad) + len(bg) < 3:
            continue
        neg = np.concatenate([bad, bg])
        # a bad reference says more than an unlabelled file: three times the weight
        wneg = [3.0] * len(bad) + [1.0] * len(bg)
        # held-out separation, k-fold on the good side and the negatives alike
        k = min(5, len(good), len(neg))
        rng = np.random.default_rng(0)
        gi, ni = rng.permutation(len(good)) % k, rng.permutation(len(neg)) % k
        hp, hn, hb = [], [], []
        for f in range(k):
            w, b = _train(torch, good[gi != f], neg[ni != f], [x for x, i in zip(wneg, ni) if i != f], dev, steps=250)
            hp += torch.sigmoid(torch.tensor(good[gi == f], device=dev) @ w + b).tolist()
            hn += torch.sigmoid(torch.tensor(neg[ni == f], device=dev) @ w + b).tolist()
            hb += [int(i) < len(bad) for i in np.where(ni == f)[0]]
        w, b = _train(torch, good, neg, wneg, dev)
        gs = torch.sigmoid(torch.tensor(good, device=dev) @ w + b).cpu().numpy()
        out["modalities"][m] = {
            "w": [round(float(x), 6) for x in w.cpu().numpy()],
            "b": float(b.cpu().numpy()[0]),
            "good": int(len(good)),
            "bad": int(len(bad)),
            "background": int(len(bg)),
            "auc": _r4(_auc(np.array(hp), np.array(hn))),
            # against its bad references alone, when it has any: the boundary people drew
            "auc_bad": _r4(_auc(np.array(hp), np.array([s for s, x in zip(hn, hb) if x]))),
            # a typical good example's score, to read a new one against
            "good_median": round(float(np.median(gs)), 4),
        }
    if not out["modalities"]:
        return {"error": "too few examples: a modality needs 3 good and 3 others"}
    path = _fp_path(workspace, fingerprint)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(out, f)
    fp_volume.commit()
    return {k: v for k, v in out.items() if k != "modalities"} | {"modalities": {m: {k: v for k, v in x.items() if k != "w"} for m, x in out["modalities"].items()}, "path": path}


@app.function(image=fp_image, gpu=GPU, volumes={"/fp": fp_volume}, timeout=300, scaledown_window=120)
def fingerprint_score(workspace: str, fingerprint: str, vectors: dict) -> dict:
    import json

    import torch

    fp_volume.reload()
    path = _fp_path(workspace, fingerprint)
    if not os.path.exists(path):
        return {"error": "this fingerprint has no model yet"}
    model = json.load(open(path))
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    scores = {}
    for m, xs in (vectors or {}).items():
        head = model["modalities"].get(m)
        X = _vectors(xs)
        if not head or not len(X):
            continue
        w = torch.tensor(head["w"], device=dev)
        scores[m] = [round(float(s), 4) for s in torch.sigmoid(torch.tensor(X, device=dev) @ w + head["b"]).tolist()]
    return {"version": model["version"], "device": torch.cuda.get_device_name(0) if dev == "cuda" else "cpu", "scores": scores, "good_median": {m: h["good_median"] for m, h in model["modalities"].items()}}


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

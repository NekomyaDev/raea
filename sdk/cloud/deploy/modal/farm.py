# raea cloud's render farm on Modal (docs/cloud-render.feature CR4-CR8): server/jobs.ts's protocol, so a CPU box's
# `raea render` (RAEA_CLOUD_GPU_URL, server/offload.ts) and the TypeScript client use it unchanged, with one render
# split across many GPUs at once. From sdk/cloud:
#
#   MODAL_PROFILE=raea modal deploy deploy/modal/farm.py
#
#   POST /v1/jobs?args=JSON[&gpus=N][&gpu=KIND][&max_usd=X]   body: the project as a tar. {id}
#   GET  /v1/jobs/ID?from=N                                    {log, next, exit?}, held up to 20 s for more
#   GET  /v1/jobs/ID/out?as=zst|gz|tar                         the files the job made or changed; the job is removed
#   GET  /v1/jobs/ID/media                                     with media=1: the job's media outputs [{path, oid, size}],
#                                                              left out of /out
#   POST /v1/jobs/ID/media {objects: [{path, oid, href, header}]}   PUTs each to its signed URL in R2 (farm_media.py)
#   POST /v1/blobs/have {objects: [{oid, size}]}                which of a project's media the farm holds: {have: [oid]}
#   PUT  /v1/blobs/OID?size=N                                   one media file into the farm's cache, kept when it
#                                                              hashes to OID (a box's project not in raea's LFS)
# A job's folder may hold .raea-cloud-media.json: its media by Git LFS object id with URLs signed for it, fetched from
# R2 through a cache on the volume keyed by hash instead of coming in the tar (docs/cloud-render.feature CR23).
#
# - `raea render X.tsx -o OUT.mp4|DIR`: frame 0 renders first on one GPU, to learn the piece's length and the price of
#   a frame; the frames then go out in runs to up to `gpus` GPUs (farm_plan.py sizes them), come back as PNGs, and the
#   coordinator (a CPU) encodes them with the piece's sound
# - `blender FILE.blend [-o OUT.mp4|DIR] [--frames A-B] [--samples N]`: Cycles on OptiX, split the same way
# - anything else: `raea ARGS` whole, on one GPU
# - max_usd: frame 0's price times the frames left; over it, the job stops before the rest start (exit 3)
# - the clipping check (RAEA_CLIP_CHECK) is off on the GPUs: it is an agent's lint, and on a server's CPU it took more
#   of a render than its frames (bench/gpus.py)

import contextlib
import json
import os
import re
import shutil
import subprocess
import time
import uuid

import modal

HERE = os.path.dirname(os.path.abspath(__file__))
CLOUD = os.path.dirname(os.path.dirname(HERE))
RAEA = os.environ.get("RAEA_VERSION", "0.1.6")
BLENDER = os.environ.get("RAEA_FARM_BLENDER", "5.3.0")
# The GPUs each kind of job takes, the first Modal has free (bench/gpus.py, 11-17): raea's frames are bound by its CPU
# (React on QuickJS), so a T4 or an A10 draws them for the least a frame; Cycles gets the most samples a dollar on an
# L40S or an RTX PRO 6000 (an A100 or H100 has no RT cores: a quarter of that). L4s were scarce (a 254 s start).
RAEA_GPUS = os.environ.get("RAEA_FARM_RAEA_GPUS", "T4,A10,L4").split(",")
BLENDER_GPUS = os.environ.get("RAEA_FARM_BLENDER_GPUS", "L40S,RTX-PRO-6000,L4").split(",")
GPUS = int(os.environ.get("RAEA_FARM_GPUS", "10"))  # Modal Starter's GPU limit; Team's is 50
ROOT = "/farm"
JOBS = f"{ROOT}/jobs"

image = (
    modal.Image.from_registry("debian:trixie-slim", add_python="3.12")
    .apt_install(
        "ca-certificates", "curl", "xz-utils", "zstd", "procps", "ffmpeg",
        "libasound2t64", "libfontconfig1", "libvulkan1", "mesa-vulkan-drivers", "vulkan-tools",
        "libxext6", "libx11-6", "libgl1", "libegl1",
        "libxi6", "libxxf86vm1", "libxfixes3", "libxrender1", "libxkbcommon0", "libsm6", "libice6",
    )
    .run_commands(
        f"set -e; f=raea-{RAEA}-linux-x86_64.tar.gz; u=https://github.com/shinyobjectz/raea/releases/download/v{RAEA}; "
        "cd /tmp && curl -fsSL -o $f $u/$f && curl -fsSL -o $f.sha256 $u/$f.sha256 && sha256sum -c $f.sha256 "
        "&& mkdir -p /opt/raea && tar -xzf $f -C /opt/raea --strip-components=1 && rm -f $f $f.sha256 "
        "&& ln -s /opt/raea/bin/raea /usr/local/bin/raea && raea --version",
        f"set -e; v={BLENDER}; curl -fsSL https://download.blender.org/release/Blender${{v%.*}}/blender-$v-linux-x64.tar.xz "
        "| tar -xJ -C /opt && ln -s /opt/blender-$v-linux-x64/blender /usr/local/bin/blender && blender -b --version | head -1",
    )
    .pip_install("fastapi[standard]==0.121.2")
    # /v1/faces: OpenCV's YuNet face detector (232 KB, CPU), for the boxes' frames check (server/frames.ts): a box has
    # no vision backend of its own (raea-perceive isn't in raea's Linux build), so a face cut by a 9:16 crop went unseen
    .pip_install("opencv-python-headless==4.11.0.86", "numpy<2.3")
    .run_commands(
        "set -e; curl -fsSL -o /opt/yunet.onnx https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/"
        "face_detection_yunet/face_detection_yunet_2023mar.onnx "
        "&& echo '8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4  /opt/yunet.onnx' | sha256sum -c"
    )
    .add_local_file(os.path.join(CLOUD, "image", "nvidia_icd.json"), "/usr/share/vulkan/icd.d/nvidia_icd.json", copy=True)
    .env({"WGPU_BACKEND": "vulkan", "NVIDIA_DRIVER_CAPABILITIES": "all", "RAEA_CLIP_CHECK": "0", "RAEA_AUDIO_MUTE": "1",
          "RAEA_MODELS": "/models/raea", "RAEA_FEED_MODELS": "/models/feed", "RAEA_LIBRARY_MODELS": "/models/library",
          # a render's sound pass, for a sound its project's store doesn't have yet: TokenSynth as
          # deploy/modal/tokensynth.py installs it, its notes on the GPU (CR25); a model the volume doesn't have fails
          # the job at once instead of installing it on a GPU's clock
          "RAEA_TOKENSYNTH": "/models/tokensynth", "RAEA_TOKENSYNTH_DEVICE": "cuda", "RAEA_AUDIO_AUTOSETUP": "0",
          # Stable Audio Open small on ONNX Runtime with CUDA, as deploy/modal/sao.py installs it (CR25)
          "RAEA_SAO": "/models/sao"})
    .add_local_python_source("farm_plan", "farm_media", "prices")
)

app = modal.App(os.environ.get("RAEA_FARM_APP", "raea-farm"), image=image)
vol = modal.Volume.from_name("raea-farm", create_if_missing=True, version=2)
# raea's Linux models (deploy/modal/models.sh), read where raea looks for them
models = modal.Volume.from_name("raea-models", create_if_missing=True, version=2)
logs = modal.Dict.from_name("raea-farm-logs", create_if_missing=True)
# the farm's token (RAEA_CLOUD_TOKEN), its own: a CPU box names it as RAEA_CLOUD_GPU_TOKEN
secret = modal.Secret.from_name(os.environ.get("RAEA_FARM_SECRET", "raea-farm"))
# OPENROUTER_API_KEY for /v1/words: raea cloud's own key, the one its library index transcribes with
words_secret = modal.Secret.from_name(os.environ.get("RAEA_FARM_WORDS_SECRET", "raea-farm-words"))
# a CPU render (the coordinator's sound pass) on lavapipe, with EGL's vendors hidden (docs/cloud.feature CL13)
CPU_ENV = {"VK_DRIVER_FILES": "/usr/share/vulkan/icd.d/lvp_icd.json", "__EGL_VENDOR_LIBRARY_DIRS": "/nonexistent"}


# NVIDIA's shader cache and Cycles' kernels on the farm's volume, so a GPU's start reuses what an earlier one compiled
CACHES = {"__GL_SHADER_DISK_CACHE": "1", "__GL_SHADER_DISK_CACHE_PATH": f"{ROOT}/cache/nvidia", "__GL_SHADER_DISK_CACHE_SKIP_CLEANUP": "1",
          "__GL_SHADER_DISK_CACHE_SIZE": str(4 << 30), "XDG_CACHE_HOME": f"{ROOT}/cache/xdg"}


# when this container's Python started, for the job's bill (farm_plan.Bill)
BOOTED = time.time()


def life():
    return {"task": os.environ.get("MODAL_TASK_ID", ""), "boot": BOOTED, "end": time.time()}


def run(cmd, cwd=None, env=None, timeout=6 * 3600):
    t = time.time()
    for d in (CACHES["__GL_SHADER_DISK_CACHE_PATH"], CACHES["XDG_CACHE_HOME"]):
        os.makedirs(d, exist_ok=True)
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env={**os.environ, "HOME": "/tmp/home", **CACHES, **(env or {})}, timeout=timeout)
    return r.returncode, r.stdout + r.stderr, time.time() - t


def snapshot(d):
    out = {}
    for base, _, files in os.walk(d):
        for f in files:
            p = os.path.join(base, f)
            try:
                s = os.stat(p)
            except OSError:  # a link that points nowhere here (a box's kits/ link to its cache): not a file of the job's
                continue
            out[os.path.relpath(p, d)] = f"{s.st_size}:{s.st_mtime_ns}"
    return out


class Log:
    """The job's output for GET /v1/jobs/ID: the coordinator is its only writer."""

    def __init__(self, id):
        self.id, self.lines = id, []

    def __call__(self, text, err=False):
        self.lines.append({"err" if err else "out": text if text.endswith("\n") else text + "\n"})
        logs[self.id] = self.lines

    def end(self, code):
        logs[f"{self.id}:exit"] = code


# a GPU stays up this long after its last run, billed (farm_plan.SCALEDOWN): at 60 s, the farm's jobs of 11-17 billed
# about five times their GPUs' working seconds
SCALEDOWN = 2


def on(gpus):
    return dict(gpu=gpus, cpu=4, memory=16384, volumes={ROOT: vol, "/models": models}, timeout=6 * 3600, max_containers=GPUS, scaledown_window=SCALEDOWN)


def kind():
    """This container's GPU by its price list name (prices.py), for the planner's estimate."""
    import prices

    name = subprocess.run(["nvidia-smi", "--query-gpu=name", "--format=csv,noheader"], capture_output=True, text=True).stdout
    return prices.kind_of(name)


@app.function(**on(RAEA_GPUS))
def raea_run(id: str, args: list[str], frames: list[int] | None = None, first: int = 0, probe: bool = False, video: bool = False) -> dict:
    """raea ARGS in the job's folder. With `frames`: those frames to the job's frames/ folder, from frame `first` on,
    as one H.264 run (frames/run-FIRST.mp4, joined by the coordinator without encoding again) when `video`, else PNGs.
    With `probe`: frame 0 and the piece's length, size and rate (its manifest). One function for all three, so the
    container that rendered frame 0 is warm for the next run."""
    vol.reload()
    work = f"{JOBS}/{id}/work"
    if probe:
        code, out, s = run(["raea", *args, "--frame", "0", "-o", "/tmp/probe.png", "--manifest", "/tmp/probe.json"], cwd=work)
        m = json.load(open("/tmp/probe.json")) if os.path.exists("/tmp/probe.json") else {}
        return {"code": code, "out": out[-3000:], "s": s, "length": m.get("length"), "fps": m.get("fps"), "w": m.get("width"), "h": m.get("height"), "kind": kind(), "manifest": m, **life()}
    if frames is None:
        code, out, s = run(["raea", *args], cwd=work)
        vol.commit()
        models.commit()  # what raea fetched on first use (RAEA_FEED_MODELS), for every container after
        return {"code": code, "out": out[-4000:], "s": s}
    dest = f"{JOBS}/{id}/frames"
    os.makedirs(dest, exist_ok=True)
    if video:
        seg = f"/tmp/run-{first:05d}.mp4"
        code, out, s = run(["raea", *args, "--at", ",".join(map(str, frames)), "-o", seg], cwd=work)
        if os.path.exists(seg):
            shutil.move(seg, f"{dest}/run-{first:05d}.mp4")
        vol.commit()
        ms = re.search(r"([\d.]+) ms a frame", out)
        return {"code": code, "out": out[-2000:], "s": s, "frames": len(frames), "ms": float(ms.group(1)) if ms else None, "kind": kind(), **life()}
    tmp = f"/tmp/{uuid.uuid4().hex}"
    code, out, s = run(["raea", *args, "--at", ",".join(map(str, frames)), "-o", tmp], cwd=work)
    # raea names --at's stills in order (frame-00000.png is the first frame asked for), beside its manifest
    made = sorted(n for n in os.listdir(tmp) if re.fullmatch(r"frame-\d+\.png", n)) if os.path.isdir(tmp) else []
    # a frame's time from when the stills were written (raea's stills path doesn't say it), the start excluded
    t = [os.stat(os.path.join(tmp, n)).st_mtime for n in made]
    ms = 1000 * (t[-1] - t[0]) / (len(t) - 1) if len(t) > 1 else None
    # the run's warm-up frames (those before `first`) are dropped
    for i, name in enumerate(made):
        if frames[i] >= first:
            shutil.move(os.path.join(tmp, name), f"{dest}/frame-{frames[i]:05d}.png")
    vol.commit()
    return {"code": code, "out": out[-2000:], "s": s, "frames": len(made), "ms": ms, "kind": kind(), **life()}


BLENDER_SETUP = """
import bpy, json, sys
s = bpy.context.scene
a = json.loads(sys.argv[sys.argv.index("--") + 1])
if a.get("samples"): s.cycles.samples = a["samples"]
if s.render.engine == "CYCLES":
    p = bpy.context.preferences.addons["cycles"].preferences
    p.compute_device_type = "OPTIX"
    p.get_devices()
    for d in p.devices:
        d.use = d.type == "OPTIX"
    s.cycles.device = "GPU"
print("RAEA_INFO " + json.dumps({"start": s.frame_start, "end": s.frame_end, "fps": s.render.fps / s.render.fps_base,
    "w": s.render.resolution_x * s.render.resolution_percentage // 100, "h": s.render.resolution_y * s.render.resolution_percentage // 100,
    "engine": s.render.engine, "samples": s.cycles.samples}))
"""


@app.function(**on(BLENDER_GPUS))
def blender_run(id: str, blend: str, start: int, end: int, opts: dict) -> dict:
    vol.reload()
    work = f"{JOBS}/{id}/work"
    dest = f"{JOBS}/{id}/frames"
    os.makedirs(dest, exist_ok=True)
    cmd = ["blender", "-b", blend, "--python-expr", BLENDER_SETUP, "-o", f"{dest}/frame-#####", "-F", "PNG", "-s", str(start), "-e", str(end), "-a", "--", json.dumps(opts)]
    code, out, s = run(cmd, cwd=work)
    # a video's frames: this run's H.264 part, encoded here on this GPU's CPUs while the others render theirs
    if not code and opts.get("video"):
        import farm_plan

        seg = f"{dest}/run-{start:05d}.mp4"
        pngs = [f"{dest}/frame-{f:05d}.png" for f in range(start, end + 1)]
        c, o, _ = farm_plan.encode(dest, seg, opts["fps"], None, run, start=start, frames=end - start + 1)
        code, out = (c, out + o) if c else (0, out)
        for p in pngs:
            if os.path.exists(p):
                os.remove(p)
    vol.commit()
    times = [int(m) * 60 + float(x) for m, x in re.findall(r"Time: (\d+):([\d.]+)", out)]
    return {"code": code, "out": out[-2000:], "s": s, "frames": end - start + 1, "render_s": times, "kind": kind(), **life()}


@app.function(cpu=2, memory=4096, volumes={ROOT: vol}, timeout=24 * 3600, secrets=[secret])
def coordinate(id: str, args: list[str], opts: dict):
    import farm_media
    import farm_plan

    log = Log(id)
    log(f"raea farm: raea {' '.join(args)}")
    vol.reload()
    work = f"{JOBS}/{id}/work"
    try:
        farm_media.fetch(work, f"{ROOT}/blobs", log)
    except Exception as e:  # a file that didn't come, or came back as another: the job doesn't run
        log(f"raea farm: {e}", err=True)
        open(f"{JOBS}/{id}/changed.txt", "w").close()  # nothing made: /out answers with an empty tar
        vol.commit()
        log.end(1)
        return
    # the media just linked (or fetched) into the job, committed before any GPU container reloads to render it: without
    # this a still on this container found the footage and every GPU run of a video didn't ("No such file", 11-25)
    vol.commit()
    before = snapshot(work)
    try:
        code = farm_plan.dispatch(args, {**opts, "id": id}, work, log, raea_run, blender_run, CPU_ENV, run, vol.reload)
    except Exception as e:  # the job's own failure, said in its log; the farm goes on
        log(f"raea farm: {e!r}", err=True)
        code = 1
    vol.commit()
    # what the job's GPU containers committed (a whole render's stills, its folder output) is only seen here after a
    # reload: without it a render to a folder came back empty while a joined .mp4, written here, came back (11-20).
    # Another job's open files in this coordinator refuse a reload (ConflictError, 11-21: a 500 for the box): try again,
    # and if it still won't, go on with what this container sees rather than lose a finished render
    for pause in (0.5, 1, 2, 4, 0):
        try:
            vol.reload()
            break
        except Exception as e:  # modal.exception.ConflictError
            if not pause:
                log(f"raea farm: the volume wouldn't reload ({e}); returning what this container sees", err=True)
            else:
                time.sleep(pause)
    after = snapshot(work)
    changed = [p for p, v in after.items() if before.get(p) != v]
    if opts.get("media"):
        # the job's media stays here until the box names where in R2 it goes (POST media); the rest comes back in /out
        media = farm_media.outputs(work, changed)
        with open(f"{JOBS}/{id}/media.json", "w") as f:
            json.dump(media, f)
        kept = {m["path"] for m in media}
        changed = [p for p in changed if p not in kept]
    with open(f"{JOBS}/{id}/changed.txt", "w") as f:
        f.write("".join(p + "\n" for p in changed))
    shutil.rmtree(f"{JOBS}/{id}/frames", ignore_errors=True)
    vol.commit()
    log.end(code)


@app.function(cpu=1, memory=4096, volumes={ROOT: vol}, secrets=[secret, words_secret], timeout=3600)
@modal.concurrent(max_inputs=32)
@modal.asgi_app()
def api():
    import asyncio

    from fastapi import FastAPI, HTTPException, Request
    from fastapi.responses import JSONResponse, PlainTextResponse, Response

    web = FastAPI()
    token = os.environ["RAEA_CLOUD_TOKEN"]

    # This container serves many requests at once, and a volume reload refuses while any of them has a file open
    # (ConflictError) and, when it goes ahead, takes /farm away from a request mid-write (PermissionError making a
    # job's folder). 11-25: StreetTalk's renders lost a finished job's results (GET out: 500) and failed to start
    # (POST jobs: 500) whenever two agents rendered together. So requests that touch the volume share it, and a
    # reload waits until none of them is in it.
    class Gate:
        def __init__(self):
            self.users = 0
            self.reloading = False
            self.cond = asyncio.Condition()

        @contextlib.asynccontextmanager
        async def use(self):
            async with self.cond:
                await self.cond.wait_for(lambda: not self.reloading)
                self.users += 1
            try:
                yield
            finally:
                async with self.cond:
                    self.users -= 1
                    self.cond.notify_all()

        async def reload(self, wait: bool = True) -> bool:
            """the volume as other containers committed it; with wait=False skipped (False) while the volume is in use"""
            async with self.cond:
                if not wait and (self.users or self.reloading):
                    return False
                await self.cond.wait_for(lambda: not self.users and not self.reloading)
                self.reloading = True
            try:
                for pause in (0.5, 1, 2, 4, 0):
                    try:
                        await vol.reload.aio()
                        return True
                    except Exception as e:  # modal.exception.ConflictError: a file open elsewhere in this container
                        if not pause:
                            print(f"raea farm: the volume wouldn't reload: {e}")
                            return False
                        await asyncio.sleep(pause)
                return False
            finally:
                async with self.cond:
                    self.reloading = False
                    self.cond.notify_all()

    gate = Gate()

    async def reloaded():
        """the coordinator's commits seen here, or a 503 the box tries again on (the job is still all here)"""
        if not await gate.reload():
            raise HTTPException(503, "the farm's volume is busy: try again")

    def check(req: Request):
        got = req.headers.get("authorization", "").removeprefix("Bearer ") or req.query_params.get("token", "")
        if got != token:
            raise HTTPException(401, "a raea cloud token is needed (Authorization: Bearer)")

    @web.get("/healthz")
    def healthz():
        return PlainTextResponse("ok")

    @web.get("/v1/health")
    def health(req: Request):
        check(req)
        return {"raea": RAEA, "blender": BLENDER, "device": {"renders": "gpu", "name": f"raea on {'/'.join(RAEA_GPUS)}, Blender on {'/'.join(BLENDER_GPUS)}, up to {GPUS} at once"}, "farm": True, "jobs": []}

    # what's said in a piece of sound, word by word, for a box's `raea words` (server/words.ts): a box has no
    # transcriber of its own (raea's is parakeet-mlx, Mac only), so the same Whisper large-v3 on OpenRouter that raea
    # cloud's library index uses (apps/app host/library/words.ts) hears it here. The body is one MP3 piece (16 kHz mono,
    # at most 10 minutes; the box cuts the pieces), the answer its words in the piece's own seconds. 11-25, Shane: "give
    # it whatever we use for indexing and libraries and sync", so agents check the words their cuts say.
    def seams(g):
        """a split screen's seams: rows (h) or columns (v) where the picture jumps across nearly all of the frame, as a
        cut between two pictures does and nothing filmed does (a caption's box spans part of it, a horizon is soft)"""
        import numpy as np

        found = []
        for axis, d in (("h", np.abs(np.diff(g, axis=0))), ("v", np.abs(np.diff(g, axis=1)).T)):
            n = d.shape[0]
            share = (d > 24).mean(axis=1)
            for i in range(int(n * 0.08), int(n * 0.92)):
                if share[i] > 0.7 and share[i] >= share[max(i - 1, 0)] and share[i] >= share[min(i + 1, n - 1)]:
                    found.append([axis, i + 1, round(float(share[i]), 2)])
        return found

    @web.post("/v1/faces")
    async def faces(req: Request):
        """the faces in a video's frames, for the boxes' frames check: the body is {"frames": [base64 JPEG, ...]} (at most
        400, any size), the answer each frame's size and its faces as [x, y, w, h, score] in that frame's pixels"""
        import base64
        import cv2
        import numpy as np

        check(req)
        body = await req.json()
        frames = body.get("frames") if isinstance(body, dict) else None
        if not isinstance(frames, list) or not frames:
            raise HTTPException(400, 'the body is {"frames": [base64 JPEG, ...]}')
        if len(frames) > 400:
            raise HTTPException(413, "at most 400 frames a call")
        out, det, size = [], None, None
        for b64 in frames:
            im = cv2.imdecode(np.frombuffer(base64.b64decode(b64), np.uint8), cv2.IMREAD_COLOR)
            if im is None:
                out.append(None)
                continue
            h, w = im.shape[:2]
            if size != (w, h):
                det, size = cv2.FaceDetectorYN.create("/opt/yunet.onnx", "", (w, h), 0.6, 0.3, 5000), (w, h)
            _, found = det.detect(im)
            out.append({"w": w, "h": h, "faces": [[round(float(f[0]), 1), round(float(f[1]), 1), round(float(f[2]), 1), round(float(f[3]), 1), round(float(f[14]), 3)] for f in (found if found is not None else [])],
                        "seams": seams(cv2.cvtColor(im, cv2.COLOR_BGR2GRAY).astype(np.int16))})
        return {"model": "yunet-2023mar", "frames": out}

    @web.post("/v1/words")
    async def words(req: Request):
        import base64
        import json as _json
        import urllib.error
        import urllib.request

        check(req)
        key = os.environ.get("OPENROUTER_API_KEY", "")
        if not key:
            raise HTTPException(503, "the farm has no OpenRouter key for words (Modal secret raea-farm-words)")
        mp3 = await req.body()
        if not mp3:
            raise HTTPException(400, "the body is the sound: an MP3, 16 kHz mono, at most 10 minutes")
        if len(mp3) > 24 << 20:
            raise HTTPException(413, "a piece is at most 24 MB: send 10-minute pieces at 32 kb/s")
        body = {"model": "openai/whisper-large-v3", "input_audio": {"data": base64.b64encode(mp3).decode(), "format": "mp3"}, "response_format": "verbose_json", "timestamp_granularities": ["word", "segment"]}
        if req.query_params.get("language"):
            body["language"] = req.query_params["language"]

        def ask():
            r = urllib.request.Request("https://openrouter.ai/api/v1/audio/transcriptions", data=_json.dumps(body).encode(), headers={"authorization": f"Bearer {key}", "content-type": "application/json", "x-title": "raea cloud words"})
            try:
                with urllib.request.urlopen(r, timeout=300) as f:
                    return 200, _json.load(f)
            except urllib.error.HTTPError as e:
                return e.code, {"error": e.read()[:500].decode("utf8", "replace")}

        code, got = await asyncio.to_thread(ask)
        if code != 200:
            raise HTTPException(502, f"OpenRouter's Whisper said {code}: {got.get('error', '')}")
        ws = [{"text": w.get("word", "").strip(), "start": float(w.get("start", 0)), "end": float(w.get("end", w.get("start", 0)))} for w in got.get("words") or [] if w.get("word", "").strip()]
        return {"model": "openai/whisper-large-v3", "duration": got.get("duration"), "text": got.get("text", ""), "words": ws}

    @web.post("/v1/jobs")
    async def start(req: Request):
        check(req)
        args = json.loads(req.query_params.get("args", "null"))
        if not isinstance(args, list) or not all(isinstance(a, str) for a in args):
            raise HTTPException(400, "args must be a JSON array of strings: raea's own arguments")
        q = req.query_params
        opts = {"gpus": min(int(q.get("gpus", GPUS)), GPUS), "gpu": q.get("gpu"), "max_usd": float(q["max_usd"]) if "max_usd" in q else None, "media": q.get("media") == "1"}
        id = str(uuid.uuid4())
        work = f"{JOBS}/{id}/work"
        async with gate.use():
            os.makedirs(work)
            tar = f"{JOBS}/{id}/in.tar"
            with open(tar, "wb") as f:
                async for chunk in req.stream():
                    f.write(chunk)
            # GNU tar reads the compression itself, refuses members with .. and strips a leading /
            x = await asyncio.to_thread(subprocess.run, ["tar", "-xf", tar, "-C", work, "--no-same-owner", "--no-same-permissions"], capture_output=True, text=True)
            os.remove(tar)
            if x.returncode:
                shutil.rmtree(f"{JOBS}/{id}", ignore_errors=True)
                raise HTTPException(400, f"the folder didn't unpack: {x.stderr.strip()}")
            await vol.commit.aio()
        await logs.put.aio(id, [])
        await coordinate.spawn.aio(id, args, opts)
        return {"id": id}

    # a project's footage by hash, sent once (11-25: StreetTalk's 1.9 GB of interviews went in every render's tar, even
    # one frame's, and three at once ran the box out of memory): the box asks which the farm has, PUTs the rest, and a
    # job's .raea-cloud-media.json names them with no href (farm_media.fetch uses them from the cache)
    @web.post("/v1/blobs/have")
    async def blobs_have(req: Request):
        check(req)
        objects = (await req.json()).get("objects") or []
        await gate.reload(wait=False)  # the volume in use: what this container sees will do (a blob it lacks is sent again)
        have = []
        async with gate.use():
            for o in objects[:5000]:
                oid, size = str(o.get("oid", "")), int(o.get("size", -1))
                blob = f"{ROOT}/blobs/{oid}"
                if len(oid) == 64 and all(c in "0123456789abcdef" for c in oid) and os.path.exists(blob) and os.path.getsize(blob) == size:
                    have.append(oid)
        return {"have": have}

    @web.put("/v1/blobs/{oid}")
    async def blob_put(oid: str, req: Request):
        import hashlib

        check(req)
        if len(oid) != 64 or any(c not in "0123456789abcdef" for c in oid):
            raise HTTPException(400, "not an object id: 64 hex characters")
        size = int(req.query_params.get("size", "-1"))
        async with gate.use():
            os.makedirs(f"{ROOT}/blobs", exist_ok=True)
            blob = f"{ROOT}/blobs/{oid}"
            if os.path.exists(blob) and os.path.getsize(blob) == size:
                return {"oid": oid, "bytes": size, "had": True}
            tmp = f"{blob}.up-{uuid.uuid4().hex[:8]}"
            h = hashlib.sha256()
            n = 0
            try:
                with open(tmp, "wb") as f:
                    async for chunk in req.stream():
                        h.update(chunk)
                        f.write(chunk)
                        n += len(chunk)
                if h.hexdigest() != oid or n != size:
                    raise HTTPException(400, f"the file came as {h.hexdigest()[:12]}… ({n} bytes), not {oid[:12]}… ({size} bytes)")
                os.replace(tmp, blob)
            finally:
                if os.path.exists(tmp):
                    os.remove(tmp)
            await vol.commit.aio()
        return {"oid": oid, "bytes": n}

    @web.get("/v1/jobs/{id}")
    async def follow(id: str, req: Request):
        check(req)
        start = int(req.query_params.get("from", 0))
        until = time.time() + 20
        while True:
            lines = await logs.get.aio(id)
            if lines is None:
                raise HTTPException(404, "no such job")
            code = await logs.get.aio(f"{id}:exit")
            if len(lines) > start or code is not None or time.time() > until:
                body = {"log": lines[start:], "next": len(lines)}
                return JSONResponse(body if code is None else {**body, "exit": code})
            await asyncio.sleep(0.5)

    @web.get("/v1/jobs/{id}/media")
    async def media_list(id: str, req: Request):
        check(req)
        if await logs.get.aio(f"{id}:exit") is None:
            raise HTTPException(404, "no such finished job")
        await reloaded()
        async with gate.use():
            p = f"{JOBS}/{id}/media.json"
            return {"objects": json.load(open(p)) if os.path.exists(p) else []}

    @web.post("/v1/jobs/{id}/media")
    async def media_send(id: str, req: Request):
        import farm_media

        check(req)
        if await logs.get.aio(f"{id}:exit") is None:
            raise HTTPException(404, "no such finished job")
        await reloaded()
        body = await req.json()
        objects = body.get("objects") if isinstance(body, dict) else None
        if not isinstance(objects, list):
            raise HTTPException(400, "objects: [{path, oid, href, header}]")
        # said in this container's log, not the job's (its log is done, and Log would start it again)
        try:
            async with gate.use():
                return await asyncio.to_thread(farm_media.upload, f"{JOBS}/{id}/work", objects, print)
        except ValueError as e:
            raise HTTPException(400, str(e))

    @web.get("/v1/jobs/{id}/out")
    async def out(id: str, req: Request):
        check(req)
        if await logs.get.aio(f"{id}:exit") is None:
            raise HTTPException(404, "no such finished job")
        await reloaded()
        pack = {"zst": ["--zstd"], "gz": ["--gzip"], "tar": []}.get(req.query_params.get("as", "zst"), ["--zstd"])
        async with gate.use():
            r = await asyncio.to_thread(subprocess.run, ["tar", *pack, "-cf", "-", "-C", f"{JOBS}/{id}/work", "--verbatim-files-from", "-T", f"{JOBS}/{id}/changed.txt"], capture_output=True)
            shutil.rmtree(f"{JOBS}/{id}", ignore_errors=True)
            await vol.commit.aio()
        for k in (id, f"{id}:exit"):
            await logs.pop.aio(k, None)
        if r.returncode:
            raise HTTPException(500, f"the results didn't pack: {r.stderr.decode().strip()}")
        return Response(r.stdout, media_type="application/octet-stream")

    return web

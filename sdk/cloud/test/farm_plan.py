# The farm's planner (deploy/modal/farm_plan.py) with stand-in GPUs: python3 test/farm_plan.py from sdk/cloud. The
# stand-ins write the frames they're asked for, so a split render is checked end to end, ffmpeg's encode included.

import json
import os
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "deploy", "modal"))
import farm_plan as fp  # noqa: E402

ok = lambda m: print(f"ok {m}")


def run(cmd, cwd=None, env=None, timeout=None):
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr, 0.1


class FakeGpu:
    """raea_run's stand-in: a 60-frame piece, frame 0 in 12 s (the start), 0.5 s a frame after."""

    def __init__(self, root, length=60):
        self.root, self.length, self.calls, self.options = root, length, [], {}

    def png(self, path):
        # a 16x16 grey PNG, made once by ffmpeg
        src = os.path.join(self.root, "one.png")
        if not os.path.exists(src):
            run(["ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=gray:s=16x16", "-frames:v", "1", src])
        shutil.copy(src, path)

    def remote(self, id, args, frames=None, first=0, probe=False, video=False):
        self.calls.append(("probe" if probe else "run", frames, first))
        self.args = getattr(self, "args", []) + [list(args)]
        if probe:
            # as raea does with two --manifest: the first wins, so the probe's own says nothing
            if args.count("--manifest") > 0:
                return {"code": 0, "out": "APPLIED  1 frames", "s": 12.5, "length": None, "fps": None, "w": None, "h": None, "manifest": {}}
            return {"code": 0, "out": "", "s": 12.5, "length": self.length, "fps": 30, "w": 1920, "h": 1080, "manifest": {"length": self.length, "fps": 30}}
        dest = os.path.join(self.root, "jobs", id, "frames")
        os.makedirs(dest, exist_ok=True)
        if video:
            # the run as one H.264 video, as raea render --at FRAMES -o run.mp4 writes it
            run(["ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=gray:s=16x16:r=30", "-frames:v", str(len(frames)), "-c:v", "libx264", "-pix_fmt", "yuv420p", f"{dest}/run-{first:05d}.mp4"])
            return {"code": 0, "out": "", "s": 12 + 0.5 * len(frames), "frames": len(frames), "ms": 500.0}
        for f in frames:
            if f >= first:
                self.png(f"{dest}/frame-{f:05d}.png")
        return {"code": 0, "out": "", "s": 12 + 0.5 * len(frames), "frames": len(frames), "ms": 500.0}

    def with_options(self, **kw):
        self.options = kw
        return self

    def starmap(self, calls, order_outputs=False, return_exceptions=True):
        return [self.remote(*c) for c in calls]


lines = []
log = lambda t, err=False: lines.append(t)

assert fp.split_out(["render", "a.tsx", "-o", "out/a.mp4", "--w", "640"]) == (["render", "a.tsx", "--w", "640"], "out/a.mp4")
assert fp.splittable(["render", "a.tsx", "-o", "a.mp4"]) and fp.splittable(["render", "a.tsx", "-o", "frames"])
assert not fp.splittable(["render", "a.tsx", "-o", "a.png"]) and not fp.splittable(["render", "a.tsx", "--frame", "3", "-o", "a.mp4"])
assert not fp.splittable(["guide", "gen"]) and not fp.splittable(["render", "a.tsx", "-o", "a.mp4", "--audio", "a.wav"])
ok("only a whole-piece render to a video or a folder is split")

# 12 s to start, 0.5 s a frame: a GPU is worth it from 96 frames (its start at most a fifth of its time)
assert fp.gpus_for(52, 0.5, 12, 10) == 1
assert fp.gpus_for(960, 0.5, 12, 10) == 10 and fp.gpus_for(480, 0.5, 12, 10) == 5
# Cycles at 40 s a frame and 20 s to start: a GPU from 2 frames on
assert fp.gpus_for(240, 40, 20, 50) == 50 and fp.gpus_for(10, 40, 20, 50) == 5
assert fp.gpus_for(0, 1, 1, 5) == 0
ok("GPUs are started only where their start is at most a fifth of their work, up to the limit")

r = fp.runs(8, 59, 4)
assert [x[1] for x in r] == [8, 21, 34, 47] and sum(len(x[0]) for x in r) == 52 and r[-1][0][-1] == 59
w = fp.runs(8, 59, 2, warm=5)
assert w[0][0][0] == 3 and w[1][0][0] == 29 and w[1][1] == 34
ok("frames split into contiguous runs, each led by its warm-up frames")

root = tempfile.mkdtemp()
work = os.path.join(root, "jobs", "j1", "work")
os.makedirs(work)
gpu = FakeGpu(root)
code = fp.raea_render(["render", "a.tsx", "-o", "out/a.mp4"], {"id": "j1", "gpu": "L4", "gpus": 10}, work, log, gpu, {}, run)
assert code == 0, lines
assert gpu.calls[0][0] == "probe" and gpu.calls[1][1] == list(range(8))
assert len(gpu.calls) == 3, gpu.calls  # 52 frames left at 0.5 s with a 12 s start: one GPU more
assert os.path.getsize(os.path.join(work, "out", "a.mp4")) > 0
frames = int(subprocess.run(["ffprobe", "-v", "error", "-count_frames", "-select_streams", "v", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0", os.path.join(work, "out", "a.mp4")], capture_output=True, text=True).stdout.strip())
assert frames == 60, frames
ok("a 60-frame render: frame 0, a timed run of 8, the rest on one more GPU, the runs joined into 60 frames")

# the render's own --manifest (as an agent passes it): the GPUs never get it, and it's written from frame 0's at the end
lines.clear()
m = FakeGpu(root)
os.makedirs(os.path.join(root, "jobs", "jm", "work"))
code = fp.raea_render(["render", "a.tsx", "-o", "out/a.mp4", "--manifest", "out/a.render.json", "--quality", "high"], {"id": "jm", "gpu": "L4", "gpus": 10}, os.path.join(root, "jobs", "jm", "work"), log, m, {}, run)
assert code == 0, lines
assert all("--manifest" not in a and "--quality" in a for a in m.args), m.args
assert json.load(open(os.path.join(root, "jobs", "jm", "work", "out", "a.render.json")))["length"] == 60
ok("a render's own --manifest isn't passed to the GPUs (the probe would lose the length), and is written at the end")

lines.clear()
long = FakeGpu(root, length=1000)
os.makedirs(os.path.join(root, "jobs", "j2", "work"))
code = fp.raea_render(["render", "a.tsx", "-o", "stills"], {"id": "j2", "gpu": "L4", "gpus": 10, "max_usd": 0.01}, os.path.join(root, "jobs", "j2", "work"), log, long, {}, run)
assert code == 3 and "over max_usd" in lines[-1] and len(long.calls) == 2, (code, lines)
ok("over max_usd, the job stops after the timed run and says the price")

code = fp.raea_render(["render", "a.tsx", "-o", "stills"], {"id": "j2", "gpu": "L4", "gpus": 10}, os.path.join(root, "jobs", "j2", "work"), log, long, {}, run)
out = os.listdir(os.path.join(root, "jobs", "j2", "work", "stills"))
assert code == 0 and len(out) == 1000 and "frame-00999.png" in out, (code, len(out))
assert len(long.calls) == 2 + 2 + 10, len(long.calls)
ok("a 1000-frame render to a folder: 10 GPUs, the frames named as raea names them")

# below the GPU limit each GPU already gets just enough frames for its start: one part each
assert fp.parts_per_gpu(19, 10, 8, 5) == 1 and fp.parts_per_gpu(992, 9, 0.22, 6) == 1
# capped by the limit, a long job goes in up to 4 parts a GPU while each part still pays for its start
assert fp.parts_per_gpu(250, 10, 8, 5) == 4 and fp.parts_per_gpu(60, 10, 8, 5) == 2
lines.clear()
os.makedirs(os.path.join(root, "jobs", "j3", "work"))
q = FakeGpu(root, length=1000)
code = fp.raea_render(["render", "a.tsx", "-o", "stills"], {"id": "j3", "gpu": "L4", "gpus": 2}, os.path.join(root, "jobs", "j3", "work"), log, q, {}, run)
out = os.listdir(os.path.join(root, "jobs", "j3", "work", "stills"))
assert code == 0 and len(out) == 1000 and q.options == {"max_containers": 2} and len(q.calls) == 2 + 8, (code, len(out), q.options, len(q.calls))
assert "on 2 L4s in 8 parts" in " ".join(lines), lines
ok("a long job on too few GPUs: 4 parts a GPU from a queue of 2 at a time, every frame back")

b = fp.Bill()
t = b.t0
b.add({"task": "a", "kind": "L40S", "boot": t - 100, "end": t + 10})  # left up by an earlier job: from this job's start
b.add({"task": "a", "kind": "L40S", "boot": t - 100, "end": t + 20})
b.add({"task": "b", "kind": "T4", "boot": t + 1, "end": t + 6})
b.add({"code": 1, "out": "no task id"})
assert "2 GPU containers up 35 s" in b.line(), b.line()
ok("the bill: each container from its boot or the job's start to its last run, plus its boot and idle seconds")

shutil.rmtree(root)

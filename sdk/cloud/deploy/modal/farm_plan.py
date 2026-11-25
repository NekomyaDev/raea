# How the farm (farm.py) splits a job: which kind of job it is, how many GPUs a render is worth, the runs of frames
# each one takes, what it will cost, and the encode at the end. No Modal here, so test/farm_plan.py runs it on a laptop
# with stand-in GPUs.

import json
import math
import os
import re
import shutil
import threading
import time

import prices

VIDEO = (".mp4", ".mov")
COMPOSITION = re.compile(r"\.(tsx|ts|jsx|js|mjs)$")
# frames rendered on the first GPU after frame 0, to time a frame before the rest go out
FIRST = 8
# a GPU's start (container, raea or Blender, the scene) kept to at most this share of what it does
SHARE = 0.2


def split_out(args):
    """The arguments without -o OUT, and OUT."""
    if "-o" not in args:
        return list(args), None
    i = args.index("-o")
    return args[:i] + args[i + 2 :], args[i + 1] if i + 1 < len(args) else None


def drop_flag(args, flag):
    """The arguments without FLAG VALUE, and VALUE (None when it isn't there)."""
    if flag not in args:
        return list(args), None
    i = args.index(flag)
    return args[:i] + args[i + 2 :], args[i + 1] if i + 1 < len(args) else None


def splittable(args):
    """A `raea render` of a composition to a video or a folder, every frame: the farm can split it."""
    if not args or args[0] != "render" or any(a in args for a in ("--frame", "--at", "--review", "--bundle", "--audio")):
        return False
    rest, out = split_out(args)
    return out is not None and (out.endswith(VIDEO) or not os.path.splitext(out)[1]) and any(COMPOSITION.search(a) for a in rest[1:])


def gpus_for(frames, frame_s, start_s, most):
    """The GPUs worth starting for `frames` frames of `frame_s` each, when each one first spends `start_s`: the most that
    keep each one's start at most SHARE of its time, never more than `most`."""
    if frames <= 0:
        return 0
    if frame_s <= 0:
        return min(most, frames)
    worth = start_s * (1 - SHARE) / SHARE  # the work (s) that makes a start worth it
    k = math.floor(frames * frame_s / worth + 1e-9) if worth > 0 else frames
    return max(1, min(most, frames, k))


# a GPU takes up to this many parts of a long job, one after another from the job's queue, so a slow card (an L4 among
# L40Ss) ends up drawing fewer frames instead of holding up the job
PARTS = 4


def parts_per_gpu(frames, k, frame_s, start_s):
    """How many parts each of `k` GPUs takes from the queue: the most (up to PARTS) that keep a part's own start
    (start_s, more than a warm GPU needs) at most SHARE of the part. Above 1 only when the GPU limit capped `k`."""
    if k <= 0 or frame_s <= 0:
        return 1
    least = start_s * (1 - SHARE) / SHARE / frame_s  # the frames a part needs for its start to be worth it
    m = 1
    while m < PARTS and frames / (k * (m + 1)) >= least:
        m += 1
    return m


def runs(first, last, k, warm=0):
    """Frames first..last (inclusive) as `k` contiguous runs, each led by `warm` frames to drop (a renderer's history,
    Solari's light, settling): [(frames to render, the first one kept)]."""
    n = last - first + 1
    k = max(1, min(k, n))
    out, at = [], first
    for i in range(k):
        size = n // k + (1 if i < n % k else 0)
        lead = max(0, at - warm)
        out.append((list(range(lead, at + size)), at))
        at += size
    return out


def price(kind, seconds, cpu=4, memory_gib=16):
    return prices.cost(kind, cpu=cpu, memory_gib=memory_gib, seconds=seconds)


SCALEDOWN = 2  # s a GPU container stays up after its last run (farm.py), billed
BOOT = 3  # s a container is billed before its Python starts: the bench's cold starts on T4, L40S, RTX PRO 6000 (3-4 s)


class Bill:
    """What Modal bills for a job's GPU containers: each one's life from its boot (or the job's start, for one an
    earlier job left up) to its last run's end, plus BOOT and SCALEDOWN, at its kind's price with 4 cores and 16 GiB."""

    def __init__(self):
        self.t0, self.tasks = time.time(), {}

    def add(self, r):
        if isinstance(r, dict) and r.get("task"):
            b, e = max(r["boot"], self.t0), r["end"]
            k, b0, e0 = self.tasks.get(r["task"], (r.get("kind"), b, e))
            self.tasks[r["task"]] = (k or r.get("kind"), min(b, b0), max(e, e0))
        return r

    def line(self):
        up = [(k or "L4", e - b + BOOT + SCALEDOWN) for k, b, e in self.tasks.values()]
        return f"raea farm: billed about ${sum(price(k, s) for k, s in up):.3f}: {len(up)} GPU containers up {sum(s for _, s in up):.0f} s in all"


def encode(frames_dir, out, fps, sound, run, start=0, frames=None):
    """The frames as OUT (H.264, high quality), with the sound when there is one; `frames` of them from `start`."""
    cmd = ["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(fps), "-start_number", str(start), "-i", f"{frames_dir}/frame-%05d.png"]
    if frames:
        cmd += ["-frames:v", str(frames)]
    if sound and os.path.exists(sound) and os.path.getsize(sound) > 44:
        cmd += ["-i", sound, "-c:a", "aac", "-b:a", "256k", "-shortest"]
    cmd += ["-c:v", "libx264", "-preset", "medium", "-crf", "16", "-pix_fmt", "yuv420p", "-movflags", "+faststart", out]
    return run(cmd)


def join(frames_dir, out, sound, run):
    """The runs' H.264 videos (run-FIRST.mp4) as OUT, one after another without encoding again, with the sound."""
    runs_ = sorted(n for n in os.listdir(frames_dir) if re.fullmatch(r"run-\d+\.mp4", n))
    listing = os.path.join(frames_dir, "runs.txt")
    with open(listing, "w") as f:
        f.writelines(f"file '{n}'\n" for n in runs_)
    cmd = ["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", listing]
    if sound and os.path.exists(sound) and os.path.getsize(sound) > 44:
        cmd += ["-i", sound, "-map", "0:v", "-map", "1:a", "-c:a", "aac", "-b:a", "256k", "-shortest"]
    cmd += ["-c:v", "copy", "-movflags", "+faststart", out]
    return run(cmd)


def keep(frames_dir, out, start=0):
    """The frames as a folder of PNGs, named as raea names them (frame-00000.png is the first)."""
    os.makedirs(out, exist_ok=True)
    for i, name in enumerate(sorted(os.listdir(frames_dir))):
        shutil.move(os.path.join(frames_dir, name), os.path.join(out, f"frame-{i:05d}.png"))


def spread(fn, kind, calls, log, what, bill=None, most=None):
    """`calls` on `fn`, at most `most` GPUs at once when there are more calls than that (each GPU takes the next call
    as it finishes one); each finished run said in the log as it lands. The first failure's code, else 0."""
    code = 0
    if most and len(calls) > most:
        fn = fn.with_options(max_containers=most)
    for r in fn.starmap(calls, order_outputs=False, return_exceptions=True):
        if isinstance(r, Exception):
            log(f"raea farm: a run failed: {r!r}", err=True)
            code = code or 1
            continue
        if bill:
            bill.add(r)
        log(f"raea farm: {r.get('frames')} {what} on a {r.get('kind') or kind} in {r['s']:.1f} s")
        if r["code"]:
            log(r["out"], err=True)
            code = code or r["code"]
    return code


def raea_render(args, opts, work, log, render, cpu_env, run, reload=lambda: None, bill=None):
    bill = bill or Bill()
    id = opts["id"]
    rest, out = split_out(args)
    # the render's own --manifest is written here once it's done (from frame 0's): left in, the probe's run had two and
    # wrote its own where the render's went, so it said no length and the job stopped unexplained (11-25: StreetTalk's
    # four cuts, each "exit 1" after frame 0); and every GPU run would have written one into the job's folder
    rest, manifest = drop_flag(rest, "--manifest")
    # the sound, on this CPU while the GPUs draw (a video's frames come back without it)
    sound = f"/tmp/{id}.wav"
    if out.endswith(VIDEO):
        t = threading.Thread(target=lambda: run(["raea", *rest, "-o", sound], cwd=work, env=cpu_env))
        t.start()
    p = bill.add(render.remote(id, rest, probe=True))
    if p["code"] or not p["length"]:
        log(p["out"], err=True)
        if not p["code"]:
            log("raea farm: frame 0 rendered but didn't say the piece's length, so its frames can't be shared out", err=True)
        return p["code"] or 1
    length, fps, kind = p["length"], p["fps"] or 30, p.get("kind") or opts.get("gpu") or "T4"
    log(f"raea farm: {length} frames at {p['w']}x{p['h']}, {fps} fps; frame 0 on a {kind} in {p['s']:.1f} s")
    video = out.endswith(VIDEO)
    n0 = min(length, FIRST)
    r0 = bill.add(render.remote(id, rest, list(range(n0)), 0, video=video))
    if r0["code"]:
        log(r0["out"], err=True)
        return r0["code"]
    frame_s = (r0["ms"] or 1000 * r0["s"] / n0) / 1000
    start_s = max(0.0, r0["s"] - frame_s * n0)
    left = length - n0
    k = gpus_for(left, frame_s, start_s, opts["gpus"])
    m = parts_per_gpu(left, k, frame_s, start_s)
    cost = price(kind, p["s"] + r0["s"] + k * m * start_s + left * frame_s)
    parts = f" in {k * m} parts" if m > 1 else ""
    log(f"raea farm: {frame_s * 1000:.0f} ms a frame, {start_s:.1f} s to start; {left} frames left on {k} {kind}s{parts}, about ${cost:.3f} in all")
    if opts.get("max_usd") is not None and cost > opts["max_usd"]:
        log(f"raea farm: about ${cost:.3f} is over max_usd ${opts['max_usd']:.3f}, so the rest don't start", err=True)
        return 3
    # a video's runs can't drop warm-up frames without encoding again, so they have none
    warm = 0 if video else opts.get("warm", 0)
    code = spread(render, kind, [(id, rest, f, at, False, video) for f, at in runs(n0, length - 1, k * m, warm)], log, "frames", bill, k) if left else 0
    if code:
        return code
    # the sound's run finished first: its open files in the job's folder refuse the reload that brings the other
    # containers' frames (11-25: "the volume wouldn't reload ... cwd of raea render")
    if video:
        t.join()
    reload()  # the other containers' frames
    frames = f"{os.path.dirname(work)}/frames"
    dest = os.path.join(work, out)
    if manifest and p.get("manifest"):
        path = os.path.join(work, manifest)
        os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
        with open(path, "w") as f:
            json.dump(p["manifest"], f, indent=2)
    if video:
        os.makedirs(os.path.dirname(dest) or ".", exist_ok=True)
        c, o, s = join(frames, dest, sound, run)
        log(f"raea farm: joined {out} in {s:.1f} s" if not c else o, err=bool(c))
        return c
    keep(frames, dest)
    return 0


def blender_info(blend, work, run):
    c, o, _ = run(["blender", "-b", blend, "--python-expr", BLENDER_INFO], cwd=work)
    m = re.search(r"RAEA_INFO (\{.*\})", o)
    return json.loads(m.group(1)) if m else None


BLENDER_INFO = """
import bpy, json
s = bpy.context.scene
print("RAEA_INFO " + json.dumps({"start": s.frame_start, "end": s.frame_end, "fps": s.render.fps / s.render.fps_base,
    "engine": s.render.engine}))
"""


def blender(args, opts, work, log, render, run, reload=lambda: None, bill=None):
    bill = bill or Bill()
    """blender FILE.blend [-o OUT.mp4|DIR] [--frames A-B] [--samples N]"""
    id = opts["id"]
    rest, out = split_out(args[1:])
    blend = next((a for a in rest if a.endswith(".blend")), None)
    if not blend:
        log("raea farm: blender needs a .blend", err=True)
        return 2
    info = blender_info(blend, work, run)
    if not info:
        log(f"raea farm: Blender couldn't open {blend}", err=True)
        return 2
    first, last = info["start"], info["end"]
    if "--frames" in rest:
        a, _, b = rest[rest.index("--frames") + 1].partition("-")
        first, last = int(a), int(b or a)
    o = {"samples": int(rest[rest.index("--samples") + 1])} if "--samples" in rest else {}
    video = bool(out and out.endswith(VIDEO))
    if video:
        o |= {"video": True, "fps": info["fps"]}
    log(f"raea farm: {blend}, frames {first}-{last} ({info['engine']}), at {info['fps']:g} fps")
    r0 = bill.add(render.remote(id, blend, first, first, o))
    if r0["code"]:
        log(r0["out"], err=True)
        return r0["code"]
    kind = r0.get("kind") or opts.get("gpu") or "L40S"
    frame_s = r0["render_s"][-1] if r0["render_s"] else r0["s"]
    start_s = max(0.0, r0["s"] - frame_s)
    left = last - first
    k = gpus_for(left, frame_s, start_s, opts["gpus"])
    m = parts_per_gpu(left, k, frame_s, start_s)
    cost = price(kind, r0["s"] + k * m * start_s + left * frame_s)
    parts = f" in {k * m} parts" if m > 1 else ""
    log(f"raea farm: frame {first} on a {kind} in {r0['s']:.1f} s ({frame_s:.1f} s rendering); {left} left on {k} {kind}s{parts}, about ${cost:.3f} in all")
    if opts.get("max_usd") is not None and cost > opts["max_usd"]:
        log(f"raea farm: about ${cost:.3f} is over max_usd ${opts['max_usd']:.3f}, so the rest don't start", err=True)
        return 3
    calls = [(id, blend, f[0], f[-1], o) for f, _ in runs(first + 1, last, k * m)] if left else []
    code = spread(render, kind, calls, log, "frames", bill, k) if calls else 0
    if code:
        return code
    reload()  # the other containers' frames
    frames = f"{os.path.dirname(work)}/frames"
    dest = os.path.join(work, out or "render")
    if video:
        os.makedirs(os.path.dirname(dest) or ".", exist_ok=True)
        c, o2, s = join(frames, dest, None, run)
        log(f"raea farm: joined {out} in {s:.1f} s" if not c else o2, err=bool(c))
        return c
    keep(frames, dest)
    return 0


def dispatch(args, opts, work, log, render, blender_render, cpu_env, run, reload=lambda: None):
    """The job: a split raea render, a split Blender render, or `raea ARGS` whole on one GPU."""
    if opts.get("gpu"):
        render, blender_render = (f.with_options(gpu=opts["gpu"]) for f in (render, blender_render))
    if args and args[0] == "blender":
        bill = Bill()
        code = blender(args, opts, work, log, blender_render, run, reload, bill)
        log(bill.line())
        return code
    bill = Bill()
    if splittable(args):
        code = raea_render(args, opts, work, log, render, cpu_env, run, reload, bill)
    else:
        r = bill.add(render.remote(opts["id"], args))
        log(r["out"], err=bool(r["code"]))
        code = r["code"]
    log(bill.line())
    return code

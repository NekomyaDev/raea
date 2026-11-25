# The farm's media by hash (deploy/modal/farm_media.py) against a local HTTP server standing in for R2's signed URLs:
# python3 test/farm_media.py from sdk/cloud. Checked: a job's inputs come by hash into the cache once (a second job
# fetches nothing, but its URL must still open the file), a wrong hash and a path out of the job are refused, its media outputs are found by type and size
# and PUT to their URLs, and outputs meant for outside the project aren't.

import hashlib
import http.server
import json
import os
import sys
import tempfile
import threading

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "deploy", "modal"))
import farm_media as fm  # noqa: E402

ok = lambda m: print(f"ok {m}")
r2 = tempfile.mkdtemp()
hits = {"GET": 0, "PUT": 0, "bytes": 0}


class R2(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        hits["GET"] += 1
        p = os.path.join(r2, self.path.strip("/"))
        if not os.path.exists(p):
            self.send_response(404)
            self.end_headers()
            return
        body = open(p, "rb").read()
        if self.headers.get("range") == "bytes=0-0":
            self.send_response(206)
            body = body[:1]
        else:
            self.send_response(200)
        self.end_headers()
        hits["bytes"] += len(body)
        self.wfile.write(body)

    def do_PUT(self):
        hits["PUT"] += 1
        if self.headers.get("x-signed") != "yes":
            self.send_response(403)
            self.end_headers()
            return
        n = int(self.headers["content-length"])
        open(os.path.join(r2, self.path.strip("/")), "wb").write(self.rfile.read(n))
        self.send_response(200)
        self.end_headers()


srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), R2)
threading.Thread(target=srv.serve_forever, daemon=True).start()
base = f"http://127.0.0.1:{srv.server_address[1]}"

take = os.urandom(300_000)
oid = hashlib.sha256(take).hexdigest()
open(os.path.join(r2, oid), "wb").write(take)
cache = tempfile.mkdtemp()


def job(objects):
    work = tempfile.mkdtemp()
    with open(os.path.join(work, fm.MANIFEST), "w") as f:
        json.dump({"objects": objects}, f)
    return work


obj = {"path": "footage/take.mov", "oid": oid, "size": len(take), "href": f"{base}/{oid}", "header": {}}

# footage the box sent the farm itself (PUT /v1/blobs, no href): used from the cache as a link, never fetched
sent = b"interview " * 5000
soid = hashlib.sha256(sent).hexdigest()
open(os.path.join(cache, soid), "wb").write(sent)
ws = job([{"path": "media/int1.MOV", "oid": soid, "size": len(sent)}])
gets = hits["GET"]
ss = fm.fetch(ws, cache, log=lambda _: None)
assert open(os.path.join(ws, "media/int1.MOV"), "rb").read() == sent and os.path.islink(os.path.join(ws, "media/int1.MOV"))
assert ss == {"objects": 1, "fetched": 0, "cached": len(sent)} and hits["GET"] == gets, ss
try:
    fm.fetch(job([{"path": "media/gone.MOV", "oid": "ab" * 32, "size": 10}]), cache, log=lambda _: None)
    raise AssertionError("footage never sent was used")
except ValueError as e:
    assert "never sent" in str(e)
ok("footage the box sent the farm itself comes from the cache as a link, and footage it never sent is refused")

w1 = job([obj])
s1 = fm.fetch(w1, cache, log=lambda _: None)
assert open(os.path.join(w1, "footage/take.mov"), "rb").read() == take
assert not os.path.exists(os.path.join(w1, fm.MANIFEST))
assert s1 == {"objects": 1, "fetched": len(take), "cached": 0}, s1
w2 = job([obj])
before = hits["bytes"]
s2 = fm.fetch(w2, cache, log=lambda _: None)
assert hits["bytes"] == before + 1 and s2["cached"] == len(take) and s2["fetched"] == 0, (s2, hits)
assert open(os.path.join(w2, "footage/take.mov"), "rb").read() == take
ok("a job's input came from R2 by hash into the cache; the next job's came from the cache (R2 sent it 1 byte, its URL checked)")

# a job that names a cached hash with a URL that doesn't open (another account's file, a made-up link) gets nothing
try:
    fm.fetch(job([dict(obj, href=f"{base}/not-signed-for-you")]), cache, log=lambda _: None)
    raise SystemExit("FAIL a cached file went to a job whose URL doesn't open it")
except Exception as e:
    assert "404" in str(e), e
ok("a cached file goes only to a job whose own signed URL opens it")

bad = dict(obj, oid="0" * 64)
try:
    fm.fetch(job([bad]), tempfile.mkdtemp(), log=lambda _: None)
    raise SystemExit("FAIL a file with the wrong hash was taken")
except ValueError:
    pass
try:
    fm.fetch(job([dict(obj, path="../escape.mov")]), cache, log=lambda _: None)
    raise SystemExit("FAIL a path out of the job was taken")
except ValueError:
    pass
ok("a file that isn't its hash, and a path out of the job, are refused")

out = tempfile.mkdtemp()
os.makedirs(os.path.join(out, "out"))
render = os.urandom(200_000)
open(os.path.join(out, "out/edit.mp4"), "wb").write(render)
open(os.path.join(out, "out/notes.json"), "w").write("{}")
os.makedirs(os.path.join(out, ".raea-cloud-out"))
open(os.path.join(out, ".raea-cloud-out/0-x.mp4"), "wb").write(render)
found = fm.outputs(out, ["out/edit.mp4", "out/notes.json", ".raea-cloud-out/0-x.mp4"])
assert [o["path"] for o in found] == ["out/edit.mp4"] and found[0]["oid"] == hashlib.sha256(render).hexdigest(), found
ok("the job's media outputs are its video, not its small JSON, nor a file meant for outside the project")

send = [dict(found[0], href=f"{base}/{found[0]['oid']}", header={"x-signed": "yes"}), {"path": "out/kept.wav", "oid": "1" * 64, "size": 1}]
u = fm.upload(out, send, log=lambda _: None)
assert u == {"uploaded": 1, "bytes": len(render)}, u
assert open(os.path.join(r2, found[0]["oid"]), "rb").read() == render
ok("the render went to its signed URL in R2; an object R2 already has (no href) wasn't sent")
srv.shutdown()

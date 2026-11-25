#!/bin/bash
# raea cloud's agent box on a Fly Sprite (docs/cloud-render.feature CR11, CR12): raea's Linux release, ACP agents and
# sdk/cloud's server on the Sprite's own HTTPS URL, its renders sent to the farm's GPUs (deploy/modal/farm.py). A Sprite
# bills the CPU and memory it actually uses, sleeps when nothing talks to it and keeps its disk, so an agent that mostly
# waits on its model costs little; the GPUs it needs come from the farm and stop when the render does. From sdk/cloud:
#
#   deploy/sprites/box.sh up NAME      the Sprite, made or completed by deploy/sprites/setup.sh (the script a bot's
#                                      box is set up by too), and its server started
#   deploy/sprites/box.sh status NAME  its URL, raea, the server's health
#   deploy/sprites/box.sh token NAME   its RAEA_CLOUD_TOKEN, for the TypeScript client
#   deploy/sprites/box.sh promote NAME the box into raea cloud's list of boxes, at the top: the one work runs on
#                                      (POST /v1/admin/boxes; its health must answer). A box below it takes over
#                                      by itself when it stops answering, so a box is never swapped by editing secrets
#   deploy/sprites/box.sh list         raea cloud's boxes in order, each one's health and build
#   deploy/sprites/box.sh key NAME     a raea agent key (repos and storage scopes) from stdin, or the clipboard, onto
#                                      the box as RAEA_AGENT_KEY: its agents clone raea projects (raea-repo clone OWNER
#                                      PROJECT DIR, deploy/git/repo.sh), push, and send renders' media by hash (CR22, CR23)
#
# RAEA_VERSION (0.1.6) and AGENTS ("kimi-code claude-acp") pick what goes on, and RAEA_CLOUD_REF takes sdk/cloud from
# raea's public repo at that commit instead of this working tree; the farm comes from
# ~/.config/raea-cloud/farm.env (RAEA_CLOUD_GPU_URL, RAEA_CLOUD_GPU_TOKEN). The Sprite's URL stays private to the org.
# Game kits need ~/.config/raea-cloud/games/raea-games-linux-x86_64.tgz (raea-decompose and unity-rs built for Linux,
# checked against its .sha256 beside it); without it the box can fetch builds but not take them apart.
set -euo pipefail

# the org raea owns, never another business's (org.sh)
. "$(dirname "$0")/org.sh"
NAME=${2:-}
[ -n "$NAME" ] || [ "${1:-}" = list ] || { echo "box: name the Sprite"; exit 2; }
RAEA_VERSION=${RAEA_VERSION:-0.1.6}
AGENTS=${AGENTS:-kimi-code claude-acp antigravity-acp}
HERE=$(cd "$(dirname "$0")/../.." && pwd)
FARM=${RAEA_FARM_ENV:-$HOME/.config/raea-cloud/farm.env}

# the Sprite's login shell warns about line editing on every exec; only the command's own status counts
quiet() { grep --line-buffered -v 'bind: warning' || true; }
x() { sprite exec -o "$ORG" -s "$NAME" -- bash -c "$*" 2>&1 | quiet; }
# a file onto the Sprite over exec's stdin (the CLI's own upload times out on large files)
put() { sprite exec -o "$ORG" -s "$NAME" -- bash -c "umask 077 && mkdir -p \"\$(dirname $2)\" && cat > $2" <"$1" 2>&1 | quiet; }

case "${1:-}" in
up)
  [ -f "$FARM" ] || { echo "box: $FARM names no farm (RAEA_CLOUD_GPU_URL, RAEA_CLOUD_GPU_TOKEN)"; exit 2; }
  sprite list -o "$ORG" | grep -qx "$NAME" || sprite create -o "$ORG" --skip-console --label raea --label raea-cloud "$NAME"
  # sdk/cloud's files from this working tree; with RAEA_CLOUD_REF set, setup.sh takes them from raea's public repo at
  # that commit instead, as a bot's box does
  # Every build is stamped (BUILD.json on the box, shown in its health): the commit, and from a working tree whether it
  # was dirty, which files, and an id from the packed files themselves; the tarball is kept under
  # ~/.config/raea-cloud/builds/ID.tgz, so a box can be made again from exactly what another one runs (RAEA_CLOUD_BUILD=ID)
  builds=$HOME/.config/raea-cloud/builds
  if [ -n "${RAEA_CLOUD_BUILD:-}" ]; then
    [ -s "$builds/$RAEA_CLOUD_BUILD.tgz" ] || { echo "box: no build $RAEA_CLOUD_BUILD in $builds"; exit 2; }
    put "$builds/$RAEA_CLOUD_BUILD.tgz" /tmp/raea-cloud.tgz
    put "$builds/$RAEA_CLOUD_BUILD.json" /tmp/raea-cloud-build.json
  elif [ -z "${RAEA_CLOUD_REF:-}" ]; then
    mkdir -p "$builds"
    pack=$(mktemp)
    COPYFILE_DISABLE=1 tar -czf "$pack" --no-mac-metadata -C "$HERE" server src image deploy/git package.json package-lock.json
    # the id: the packed files' own contents (names and bytes, not their times)
    id=$(cd "$HERE" && find server src image deploy/git package.json package-lock.json -type f -not -name '.DS_Store' -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -c1-12)
    commit=$(git -C "$HERE" rev-parse HEAD)
    dirty=$(git -C "$HERE" status --porcelain -- . | sed 's/^...//' | tr '\n' ' ')
    printf '{"build":"%s","commit":"%s","dirty":%s,"changed":"%s","from":"%s"}\n' "$id" "$commit" "$([ -n "$dirty" ] && echo true || echo false)" "${dirty% }" "$(hostname -s)" > "$builds/$id.json"
    cp "$pack" "$builds/$id.tgz"
    echo "box: build $id (commit ${commit:0:10}$([ -n "$dirty" ] && echo ", dirty: ${dirty% }"))"
    put "$pack" /tmp/raea-cloud.tgz
    put "$builds/$id.json" /tmp/raea-cloud-build.json
    rm -f "${pack:?}"
  fi
  put "$FARM" /tmp/farm.env
  games=${RAEA_GAMES_TGZ:-$HOME/.config/raea-cloud/games/raea-games-linux-x86_64.tgz}
  if [ -s "$games" ]; then
    if [ -s "$games.sha256" ] && ! shasum -a 256 -c --status "$games.sha256"; then echo "box: $games doesn't match its .sha256"; exit 1; fi
    put "$games" /tmp/raea-games.tgz
  fi
  # A box with agents running isn't rebuilt under them: setup.sh restarts the server, which cuts their runs off
  # (11-25: a StreetTalk project failed mid-run). Waits for none, a quarter of an hour at most; RAEA_BOX_FORCE=1
  # rebuilds anyway (raea cloud then resumes the cut-off runs, projects.ts resumeRequest).
  if [ "${RAEA_BOX_FORCE:-}" != 1 ]; then
    for i in $(seq 1 90); do
      n=$(x ". ~/.raea-cloud.env 2>/dev/null; curl -fsS -m 5 -H \"authorization: Bearer \$RAEA_CLOUD_TOKEN\" localhost:8080/v1/health 2>/dev/null" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("running", 0))
except Exception: print(0)')
      [ "${n:-0}" = 0 ] && break
      [ "$i" = 90 ] && { echo "box: $NAME still has $n agents running: not rebuilt (RAEA_BOX_FORCE=1 to cut them off)"; exit 1; }
      [ "$i" = 1 ] && echo "box: $NAME has $n agents running: waiting for them to finish"
      sleep 10
    done
  fi
  # the box itself, set up by the one script a bot's box is set up by too
  if ! sprite exec -o "$ORG" -s "$NAME" -- bash -s -- "$NAME" "$RAEA_VERSION" "$AGENTS" "${RAEA_CLOUD_REF:-}" < "$(dirname "$0")/setup.sh" 2>&1 | quiet; then
    echo "box: setup.sh failed on $NAME"; exit 1
  fi
  "$0" status "$NAME"
  ;;
status)
  sprite url -o "$ORG" -s "$NAME" 2>&1 | quiet
  x "/opt/raea/bin/raea --version; . ~/.raea-cloud.env; curl -fsS -H \"authorization: Bearer \$RAEA_CLOUD_TOKEN\" localhost:8080/v1/health | head -c 600; echo"
  ;;
token) x ". ~/.raea-cloud.env; echo \$RAEA_CLOUD_TOKEN" ;;
promote | list)
  # raea cloud's staff key: RAEA_CLOUD_ADMIN_KEY, else the Mac's keychain (raea-cloud-prod-staff-key)
  api=${RAEA_CLOUD_API:-https://api.raea.cloud}
  key=${RAEA_CLOUD_ADMIN_KEY:-$(security find-generic-password -s raea-cloud-prod-staff-key -w 2> /dev/null || true)}
  [ -n "$key" ] || { echo "box: no staff key (RAEA_CLOUD_ADMIN_KEY, or raea-cloud-prod-staff-key in the keychain)"; exit 2; }
  if [ "$1" = promote ]; then
    url=$(sprite url -o "$ORG" -s "$NAME" 2>&1 | sed -n 's/^URL: *//p')
    tok=$(x ". ~/.raea-cloud.env; echo \$RAEA_CLOUD_TOKEN" | tail -1 | tr -d '[:space:]')
    [ -n "$url" ] && [ ${#tok} -ge 32 ] || { echo "box: couldn't read $NAME's address or token"; exit 1; }
    body=$(python3 -c 'import json,sys; print(json.dumps({"name": sys.argv[1], "url": sys.argv[2], "token": sys.argv[3], "first": True}))' "$NAME" "$url" "$tok")
    curl -fsS -X POST -H "authorization: Bearer $key" -H 'content-type: application/json' --data-binary @- "$api/v1/admin/boxes" <<< "$body" > /dev/null || { echo "box: raea cloud refused $NAME"; exit 1; }
  fi
  curl -fsS -H "authorization: Bearer $key" "$api/v1/admin/boxes" | python3 -c '
import json, sys
for b in json.load(sys.stdin)["data"]:
    bd = b.get("build") or {}
    print(("* " if b["runs_work"] else "  ") + b["name"].ljust(10), b["note"].ljust(22), "build", bd.get("build", "?"), "commit", str(bd.get("commit", "?"))[:10], "dirty" if bd.get("dirty") else "", "raea", bd.get("raea", "?"))'
  ;;
key)
  # never echoed: from a pipe, or the clipboard when there's none
  if [ -t 0 ]; then k=$(pbpaste); else k=$(cat); fi
  k=$(printf %s "$k" | tr -d '[:space:]')
  case "$k" in raea_ak_*) ;; *) echo "box: that isn't a raea agent key (raea_ak_...)"; exit 2 ;; esac
  f=$(mktemp)
  (umask 077; printf 'RAEA_AGENT_KEY=%s\n' "$k" >"$f")
  put "$f" /tmp/agent.env
  rm -f "${f:?}"
  x "umask 077; grep -v '^RAEA_AGENT_KEY=' ~/.raea-cloud.env > ~/.raea-cloud.env.new; cat /tmp/agent.env >> ~/.raea-cloud.env.new; rm /tmp/agent.env; mv ~/.raea-cloud.env.new ~/.raea-cloud.env \
     && sprite-env services restart raea-cloud >/dev/null && echo 'box: agent key on, the server restarted'"
  ;;
*)
  sed -n '2,16p' "$0"
  exit 2
  ;;
esac

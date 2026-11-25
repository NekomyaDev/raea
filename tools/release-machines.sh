#!/bin/bash
# The engine's release check run on real machines, hard pass and all (tools/release-check.sh with RAEA_CHECK_HARD=1):
# a release's Windows zip on the Windows PC, on its own GPU, and its Linux tarball on the Linux box. release.yml's
# runners check each archive once, on a software GPU on Windows; what people reported from their own machines (a folder
# with a space or an accent in its name, an AMD or NVIDIA GPU, a crash one run in many, two renders at once) only shows
# on a machine like theirs. This is the engine's half of the app's tools/raea-app/everywhere.sh (raea-platform).
#   tools/release-machines.sh VERSION [linux windows]        e.g. tools/release-machines.sh 0.1.10-rc3
# The archives come from the release (kept in ~/.cache/raea-app/engine-VERSION, as everywhere.sh keeps them), go to each
# machine, are checked there and removed. One line per machine at the end; exit 1 names what failed.
# RAEA_WIN_CHECK_HOST names the PC; RAEA_LINUX_BOX the Sprite; RAEA_CHECK_REPEATS how many times the one render repeats (40).
set -uo pipefail
version=${1:?usage: release-machines.sh VERSION [linux windows]}
version=${version#v}
shift
machines=("$@"); [ ${#machines[@]} -gt 0 ] || machines=(linux windows)
REPO=shinyobjectz/raea
root=$(cd "$(dirname "$0")/.." && pwd)
cache="$HOME/.cache/raea-app/engine-$version"; mkdir -p "$cache"
logs=${RAEA_MACHINES_LOGS:-$(mktemp -d)}; mkdir -p "$logs"
repeats=${RAEA_CHECK_REPEATS:-40}
archive() {
  local name=$1
  [ -f "$cache/$name" ] || gh release download "v$version" -R "$REPO" -p "$name" -p "$name.sha256" -D "$cache" --clobber >&2 || return 1
  (cd "$cache" && shasum -a 256 -c "$name.sha256" >/dev/null) || { echo "FAIL $name doesn't match its .sha256" >&2; return 1; }
  echo "$cache/$name"
}
# the check and what it reads beside itself (release/win-bundle.files), as one small tarball
kit="$logs/kit.tar"
tar -cf "$kit" -C "$root/tools" release-check.sh release/win-bundle.files || exit 1

linux() {
  local a; a=$(archive "raea-$version-linux-x86_64.tar.gz") || return 1
  # shellcheck disable=SC1091
  . "$root/sdk/cloud/deploy/sprites/org.sh" || { echo "FAIL no Sprite org"; return 1; }
  local S=(-o "$ORG" -s "${RAEA_LINUX_BOX:-raea-linux}") dir="raea-engine-check-$(date +%s)-$$"
  sprite exec --no-stdin "${S[@]}" -- bash -c "mkdir -p ~/$dir" || { echo "FAIL the Linux box isn't reachable"; return 1; }
  local want got
  want=$(wc -c < "$a" | tr -d ' ')
  got=$(sprite exec "${S[@]}" -- bash -c "cat > ~/$dir/raea.tar.gz && wc -c < ~/$dir/raea.tar.gz" < "$a" | tr -dc 0-9)
  [ "$got" = "$want" ] || { echo "FAIL the archive didn't reach the Linux box whole (${got:-0} of $want bytes)"; return 1; }
  sprite exec "${S[@]}" -- bash -c "tar -xf - -C ~/$dir" < "$kit" || { echo "FAIL copying the check"; return 1; }
  echo "machines: the check is running on the Linux box"
  # (no stdin of ours held open: a check started with it open sits silent, 11-25; under Xvfb, as release.yml runs it:
  # the app check opens a window, and the box has no screen)
  sprite exec --no-stdin "${S[@]}" -- bash -c "cd ~/$dir && RAEA_CHECK_HARD=1 RAEA_CHECK_REPEATS=$repeats timeout 2400 xvfb-run -a -s '-screen 0 1280x1024x24' bash release-check.sh ~/$dir/raea.tar.gz > check.out 2>&1 < /dev/null; code=\$?; tail -40 check.out; cd ~ && rm -rf ~/$dir; exit \$code"
}

windows() {
  local a; a=$(archive "raea-$version-windows-x86_64.zip") || return 1
  local host=${RAEA_WIN_CHECK_HOST:-cuda-box} dir="raea-engine-check-$(date +%s)-$$"
  ssh -o ConnectTimeout=15 "$host" "mkdir $dir" >/dev/null || { echo "FAIL the PC isn't reachable over ssh"; return 1; }
  # shellcheck disable=SC2064
  trap "ssh -o ConnectTimeout=15 '$host' 'rmdir /s /q $dir' >/dev/null 2>&1" RETURN
  local free
  free=$(ssh "$host" "powershell -NoProfile -Command [math]::Floor((Get-PSDrive C).Free/1MB)" | tr -dc 0-9)
  [ "${free:-0}" -ge 3000 ] || { echo "FAIL the PC has ${free:-?} MB free on C:, and the check needs 3000"; return 1; }
  scp -q "$a" "$host:$dir/raea.zip" && scp -q "$kit" "$host:$dir/kit.tar" || { echo "FAIL copying the archive to the PC"; return 1; }
  # the PC's ssh shell is cmd, which takes `>` and `&&` in a command for itself: the check's line goes as a file
  printf 'cd ~/%s && tar -xf kit.tar && RAEA_CHECK_HARD=1 RAEA_CHECK_REPEATS=%s RAEA_CHECK_DIR="$PWD/c" bash release-check.sh "$PWD/raea.zip" > check.out 2>&1; code=$?; tail -40 check.out; exit $code\n' "$dir" "$repeats" > "$logs/win-run.sh"
  scp -q "$logs/win-run.sh" "$host:$dir/run.sh" || { echo "FAIL copying the check to the PC"; return 1; }
  echo "machines: the check is running on the PC"
  ssh "$host" "\"C:\\Program Files\\Git\\bin\\bash.exe\" $dir/run.sh"
}

echo "machines: the engine $version on: ${machines[*]} (logs: $logs)"
pids=()
for m in "${machines[@]}"; do
  case $m in linux | windows) ;; *) echo "machines: linux or windows, not $m"; exit 2 ;; esac
  "$m" > "$logs/$m.log" 2>&1 &
  pids+=($!)
done
failed=()
for i in "${!machines[@]}"; do
  m=${machines[$i]}
  if wait "${pids[$i]}"; then
    echo "machines: $m ok  $(grep -h 'the hard pass too' "$logs/$m.log" | tail -1)"
  else
    echo "machines: $m FAILED  $(grep -h -E '^(FAIL|release: )' "$logs/$m.log" | tail -1)"; failed+=("$m")
  fi
done
[ ${#failed[@]} -eq 0 ] || { echo "FAIL ${failed[*]} (engine $version; logs in $logs)"; exit 1; }
echo "ok the engine $version on ${machines[*]}"

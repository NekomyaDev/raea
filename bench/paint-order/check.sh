#!/bin/bash
# Two paint bugs, each held to what Chrome draws. Exits 1 on a failure.
#   #241 z.html: a stacking context's z-index: -1 child goes under its text. White pixels (the word) must be there:
#        before the fix the box covered the word and the frame had none.
#   #240 miter.html: a stroked rounded rect under a negative viewBox origin. Nothing blue left of the outline (x 0-60,
#        y 84-130: the outline starts at x 92), and the outline's top-left corner is round (dark at its square corner).
#   bench/paint-order/check.sh      (RAEA_BIN: the raea to run, default target/release/raea)
cd "$(dirname "$0")/../.." || exit 2
BIN=${RAEA_BIN:-$PWD/target/release/raea}
export RAEA_SIGN=0 RAEA_MARK=0 RAEA_EXPORT_CACHE=0
OUT=$(mktemp -d "${TMPDIR:-/tmp}/raea-paint-order.XXXXXX")
trap 'rm -rf "${OUT:?}"' EXIT
status=0
for p in z miter; do
  "$BIN" render "bench/paint-order/$p.html" -o "$OUT/$p.png" > "$OUT/$p.log" 2>&1 || { echo "FAIL render $p (log below)"; sed 's/^/    /' "$OUT/$p.log"; exit 1; }
done
# how many pixels of a region are near a colour
count() { magick "$OUT/$1.png" -alpha off -crop "$2" +repage -fuzz "${4:-6}%" -fill black +opaque "$3" -fill white -opaque "$3" -colorspace Gray -format "%[fx:int(mean*w*h+0.5)]" info:; }
white=$(count z 600x300+0+0 "#ffffff")
(( white > 2000 )) || { echo "FAIL #241: $white white pixels, the word is under its z-index: -1 child"; status=1; }
box=$(count z 600x300+0+0 "#10203C" 3)
(( box > 3000 )) || { echo "FAIL #241: $box pixels of the box behind the word"; status=1; }
bar=$(count miter 60x46+0+84 "#3c82f0")
(( bar == 0 )) || { echo "FAIL #240: $bar blue pixels left of the outline (the miter join's bar)"; status=1; }
corner=$(count miter 6x6+91+99 "#3c82f0")
(( corner == 0 )) || { echo "FAIL #240: the outline's top-left corner is square ($corner blue pixels at its corner)"; status=1; }
edge=$(count miter 200x6+400+97 "#3c82f0")
(( edge > 300 )) || { echo "FAIL #240: the outline's top edge isn't drawn ($edge pixels)"; status=1; }
[ $status = 0 ] && echo "PASS paint order: #241 text over a negative z-index child, #240 a miter join after an arc"
exit $status

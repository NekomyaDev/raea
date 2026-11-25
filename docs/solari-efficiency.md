# Solari, forked: where MALL RAT's frame time goes (11-25)

Measured on MALL RAT's own takes (Raea/anchor-tenant, `tools/solari-sweep.sh`, scored by `out/lab/pf/score.sh`): each
lever moved alone from the take's settings, 240 frames, wall time a frame, scored SSIM/PSNR against the take as it
renders (pf0) on the finished VHS look at 1440 x 1080. pfR is pf0 drawn again: its numbers are the noise floor (and wall
time itself moves about 10% run to run). The sweep stopped partway: t8 has four levers.

| lever | t2 ms (pf0 204) | t2 ssim / psnr | t4 ms (pf0 82) | t4 ssim / psnr |
|---|---|---|---|---|
| pfR, the floor | 185 | 0.991 / 37.2 | 80 | 0.989 / 42.6 |
| pfB, 1 bounce | 186 | 0.990 / 36.2 | **64** | 0.988 / 41.0 |
| pfS, half the samples (8/4) | 194 | 0.988 / 32.6 | 75 | 0.987 / 40.5 |
| pfF, firstBounce 0.5 | 161 | 0.989 / 35.8 | 61 | 0.984 / 38.8 |
| pfC, Solari at 0.67 scale | 149 | 0.986 / 35.8 | 52 | 0.964 / 34.3 |
| pfH, Solari at 0.5 | 154 | 0.984 / 35.1 | 33 | 0.959 / 35.0 |
| pfT, Solari at 0.35 | 153 | 0.980 / 33.8 | 25 | 0.950 / 33.1 |
| pfD, whole frame 720 x 540 | 180 | 0.983 / 33.4 | 25 | 0.967 / 34.2 |
| pfE, whole frame 960 x 720 | 165 | **0.987 / 36.6** | **37** | **0.978 / 38.6** |

Solari's own GPU time (RAEA_SOLARI_TIMES) on t2: 18.5 ms at 0.5 scale (initial path 14.6, spatial and shade 2.7, world
cache 1.1, light tiles 0.1), 31.7 ms at 960 x 720; by pixel count about 70 ms at full size.

## What it says

1. **t2 is not Solari's.** At 0.35 scale Solari takes 11 ms and the frame still takes 153. About 140 ms a frame in the
   mall court goes to the rest: the composition's scene each frame (hundreds of LOD pieces, 88 baked water frames, the
   arcade), raster, the look's FramePass chain and the encode. That is the biggest saving left, and it needs a CPU
   and GPU profile of one t2 frame (xctrace), not a Solari change.
2. **Render the tape at 960 x 720.** The VHS look keeps about 320 x 240 a field, so the whole frame at 960 x 720 (pfE)
   is the best lever measured: t4 55% faster at SSIM 0.978, t2 about 15% faster, nearly at the floor. It beats scaling
   Solari alone (pfC) on both time and picture, because raster and the look shrink with it.
3. **One bounce is free in picture.** pfB sits at the floor on all three takes and saves 20% on t4: the world cache
   already carries the light past the first bounce.
4. **Fewer samples are not.** Halving spp saves little time (the cost is per pixel, not per sample, at these counts)
   and costs PSNR (t2 37.2 to 32.6). The other way round, the noise notes on v3 (21, 22) are answered with more samples
   on t5 and t8 only.
5. t8's pfS and pfF score PSNR 19.8. Frame by frame it isn't a timing slip: pfS starts at 10 dB and climbs steadily to
   39 over the 240 frames (pfR holds 35-42 throughout). With fewer samples t8's light starts far off and takes about
   4 s to settle, most likely through its short world-cache life (`cache: { frames: 6 }`, set so the power cut drops
   the light at once). In a take with a short cache, cutting samples costs seconds of wrong light, not noise.

## The quality fix: light selection by power, not by count

`generate_random_light_sample` (scene/sampling.wesl) picks a light source uniformly: `light_id = rand_range_u(light_count)`,
and every pdf divides by `light_count` (sampling.wesl 131-175, realtime/restir.wesl 330, realtime/initial_path.wesl 370).
Each emissive mesh, each directional light and each sky portal is one source. So:
- a dim exit sign gets as many samples as the sun, and a scene with 300 emissive pieces gives the sun 1/300 of them:
  noise where the sun matters (v3 notes 7, 21, 22);
- a LOD swap or a set mounted mid-shot changes the count, so the sun's share jumps and the denoiser shows a one-frame
  2-5x brightness step (t2 v3: frames 1042, 2263, 3960, 4315, each a LOD swap). The composition now holds LOD levels a
  take and mounts sets for the whole take, which works round it, not through it.

Two ways to fix it in the fork, smallest first:
- **A fixed share for the sky.** Directional lights and portals get a fixed probability q (0.5) split among them,
  emissive meshes share 1 - q. The count of emissive pieces then never moves the sun's share. Needs the two counts in
  a uniform and the sources kept in two runs (LightIndex's swap_remove keeps no order today), and the five pdf sites
  changed to read the share by kind.
- **Power-weighted (an alias table).** Each source weighted by its power (emissive luminance x area; a directional
  light's illuminance; a portal's sky radiance x area), sampled in O(1) through an alias table rebuilt when the light
  set changes. GpuLightSource grows from {kind, id} to {kind, id, pmf, threshold, alias}. The emissive-hit pdf (MIS in
  the path tracer and initial_path) needs the hit's source pmf, so the instance needs its light index. ReSTIR DI's
  light tiles inherit it for free, since they draw from generate_random_light_sample.

Either needs a release build and a check on t2's four swap frames and t8; neither was built while MALL RAT v4 held the
GPU.

## The splotches: the world cache's history, and a bake (11-25)

Shane on v4: "noisy splotchy thing in the lights and reflections ... it's definitely the world cache, can we bake it?"
Each cell takes one bounce ray an update and blends it over a history capped at 32, but the adaptive cut
(`alpha = |lum delta| / lum`, world_cache_update.wesl blend_new_samples) reads a cell's own noise as the light changing
and holds the history near 10. Ten one-ray samples is the blotch: cell-sized patches that crawl.

Built in the fork (vendor/bevy_solari, uncommitted, lane solari-bake to come):
- `world_cache_steady`: the blend keeps the whole history up to `cache.frames`, with no adaptive cut, and each update's
  ray seeded by the cell's sample count, so one cell's rays don't repeat. `cache.steady` in the SDK.
- `world_cache_update_iterations`: N cache updates a frame. `cache.iterations` in the SDK.
- `world_cache_keep`: no decay, so cells never die. `world_cache_save` / `world_cache_load`: the cache buffer
  written to a file (bake.rs: a copy to a MAP_READ buffer, mapped and written before the next frame's pipelines) or
  read into it and held (the whole world-cache stage skipped). `cache.bake: { iterations, save }` and
  `cache.load` in the SDK.

The bake doesn't fit. A cell's key holds its LOD band (stochastic between bands), its quantised position after a
±half-cell jitter and a plane slice along the normal, so one surface seen from a moving camera makes many keys. With
nothing decaying, t2's whole take filled all 2^20 slots, and so did each 6 s window of it (13 windows, each 100%).
A cache that is full loses cells, and a loaded cache missing cells has holes. A bake would need larger cells, no
jitter, or a cache sized to the take. So the live steady history is tried first: it gives the same convergence on
the cells the camera sees (they are read each frame, so never die) with no file and no capacity limit. It costs a lag
on light that moves, as long as the history: t8's power cut needs a short `frames`, as it has today.

## The cache's level drifts with time and resolution (11-25)

The same t2 frame (1188), median scene luminance: a cold single frame at 1440 x 1080 reads 0.0189 and at 160 x 240
(the meter) 0.0177. Inside the long renders, the view reads 0.130 (7x) and the meter 0.0034 (0.2x). So the cache's
long-run level depends on how many pixels feed it, and the camcorder's auto-exposure, metered at 160 x 240, set the
final 14-40x too bright: Shane's overexposed daylight. The path tracer at t2's hold frame (t2-pt) says which is right.

## What fixes the patches, measured (11-25)

The patches are **boiling**, the industry's word for denoised traced light's soft patches crawling over still surfaces
(NRD, RTXDI, GI-1.0, the ReSTIR papers, Lumen). raea now uses it (docs/skills.glossary.md, the Solari guide's "Boiling");
`raea light steady`'s blotch is its size in one held frame.

What others do about it that raea hasn't tried yet:
- a longer denoiser history: NRD says history "can be bumped up to 60 frames relatively safely" to suppress residual
  boiling, and Lumen's guides raise `MaxFramesAccumulated` (10 -> 16) before adding rays. MetalFX's history is the
  next lever here;
- blue-noise sampling (NRD: "reduced residual boiling in areas with acceptable sampling quality"), against Solari's white
  hash noise;
- a larger ReSTIR temporal cap with decorrelating mutations (Sawhney et al. 2024: boiling from hard indirect light is
  resolved by a greater M cap, and mutations remove the blotches a larger cap brings), or RTXDI's boiling filter
  (which loses energy);
- for an offline render, a noise pattern held still across frames (Octane's "static noise"): a held shot can't boil,
  though its grain stops moving.

Measured with `raea light steady` (frame held, its light's movement over time; limit 1.5% blotch, worst tile 4.5%) on
MALL RAT t2 frame 1188 (150 held, 75 measured), and the bench kitchen (STEADY 0.63 today). t2's runs of one setup spread
by 0.1-0.4, so each setup that matters was run twice.

| setup on t2 | blotch | verdict |
|---|---|---|
| today (1 path, ReSTIR, live cache) | 2.84-3.09 | fails |
| the controls: raster, and the look's passes off | 0.0, 3.04 | the patches are Solari's |
| world cache baked and frozen | 2.55 | the cache is not the main cause |
| denoiser off / its guides off | grain 17.6, 2.9 | MetalFX passes the patches, doesn't make them |
| ReSTIR off; its confidence cap 32; spatial radius 8 | 2.13; 3.03; 2.98 | ReSTIR adds, its settings don't fix |
| 2x/4x direct-light samples; 64 small emissives removed; light selection by a sky share | no change | not the direct light |
| denoiser and ReSTIR off, cache frozen: the raw light | 98% per-pixel grain, white in space and time | one path's bounce has a sd as large as its mean |
| **paths 4 / 8** (new: N initial paths a pixel, RIS-merged) | **1.89 / 2.10** | halves it |
| paths 8 + frozen cache | 1.45, 1.53 | on the line |
| paths 8 + steady cache (1024 history, 8 updates) | 1.65, 1.69 | |
| paths 4-8 + steady + still-camera query (no jitter, one LOD) | 1.66-1.88 | no better |
| paths 8, ReSTIR off, steady / frozen | 1.60, 1.69 / 1.54 | |
| paths 8 + frozen, camera jitter off | 1.31 (the one pass) | jitter isn't the floor |

Bias: the kitchen's mean at 4 paths against 1 is +0.0015 stops (per-pixel median 0); with the steady cache and still
query +0.0013. Glade's moving shot (the still query off while it moves): flicker 0.708 against 0.702. Cost on t2: a
150-frame hold 50 s at 1 path, 82-94 s at 4, 112 s at 8; glade 1.72 -> 2.47 s a still at 4.

So: `paths` is the lever that ships (default 1; MALL RAT's takes set it). It takes t2 from about 3.0 to 1.6-1.9, which
still fails the gate. The rest is the world cache: a frozen one reads 0.1-0.3 lower than any live one at the same paths,
and the live cache made to converge (steady history, more updates, the still-camera query) doesn't close the gap. Left
for the next release: a cache that converges like the frozen one (cells kept for a held shot without the 2^20 overflow:
a bigger or per-take cache, or ReSTIR GI's reservoirs carrying the cache's light), then t2 again.

Built in the fork, all off unless asked: `paths` (SolariLighting::paths_per_pixel, RAEA_SOLARI_PATHS), `cache.steady`
and `cache.iterations`, the still-camera query (with `cache.steady`, RAEA_SOLARI_STEADY=1), `cache.bake`/`cache.load`
(experimental: a whole take overflows 2^20 cells), light selection by a fixed sky share (RAEA_SOLARI_SKY_SHARE, 0 =
upstream), ReSTIR's spatial radius (RAEA_SOLARI_SPATIAL_RADIUS, 30).

## Telling the movement apart, and what each kind was (11-25, afternoon)

`raea light steady` now tells the kinds of movement in a held frame apart (engine/light/src/kinds.rs). It
reads each 32 px square over the last 75 held frames and splits its light four ways:
- **settling**: the square's straight-line trend over the window. The light is still building, which a real render's
  lead-in covers. It's reported as "unsettled" over p95 5%, not failed;
- **boiling**: what's left once the trend is removed, where it's slow (lag-1 correlation 0.5 or more);
- **jitter**: what repeats on a 2-16 frame cycle, the camera's sub-pixel jitter showing through;
- **flicker**: what is fast with no cycle, such as a firefly, a sparkle in a reflection, or two faces at one depth.

Each kind has its own limit: boiling rms 1.5% with its worst square under three times that, jitter and flicker worst
5%, and drift 0.05 stops. steady-kinds.png maps them: red is jitter or flicker, green is boiling, blue is settling.
Before, a single number mixed them, and the fixes differ.

What each turned out to be on MALL RAT:

| hold | before | change | after |
|---|---|---|---|
| t1 1699 (silhouettes against the sky) | jitter worst 73%, all 3.73 | the denoiser after the sky and forward meshes (`late`) | jitter worst 3.8%, all 0.76: STEADY |
| t2 1188 (the court, 4 paths) | jitter worst 10% on the shop signs | `late` + emissive "taa" | jitter 0.09%, worst 2.8% |
| t2 1188 | boiling 1.27%, worst 5-7% at the frame's right edge | ReSTIR's spatial neighbours reflected back into the frame, not clamped to its border (restir.wesl) | boiling 1.21%, worst 4.25%: STEADY (t1 425: 0.58) |
| kitchen | 0.63 all, grain 1.38 | `late` | 0.40 all, grain 0.55 |

- **The edges stepping with the jitter: the sky and forward meshes were drawn after the denoiser.** MetalFX ran
  before Bevy's main opaque pass (metalfx.rs `denoise`). That pass then drew the Environment's background and every
  forward mesh with the jittered camera, and nothing resolved them. Each silhouette against the sky stepped a pixel
  a frame on the 8-frame Halton cycle. `SolariSettings.late` (TSX `late`, RAEA_SOLARI_DENOISE_LATE) runs it after
  the main pass instead. The guides pass already marks a pixel with no G-buffer (sky, forward meshes) undenoised
  (mask 1) but still gathered.
- **The shop signs' edges: MALL RAT's `emissive="split"`** puts each emitter's light back after the denoiser frame by
  frame, so emissive edges keep the jitter. The default "taa" mode gathers them over frames.
- **A soft knee around MetalFX** (`knee`, RAEA_METALFX_KNEE; logarithmic above the white at preexposure/16) was
  tried first, on the theory that the resolve averaged bright edges in linear light. It changed nothing at t1 1699
  (73% either way), because the sky wasn't going through the resolve at all. It keeps the kitchen's mean within
  0.004 stops. It's kept for now, default 1.
- **Not the cache's update rate or cell life.** Every cell updated every frame (`cache.updates` 2,000,000) read 1.98
  and 1.88 against 1.89. Cells kept for 600 frames unqueried (`cache.life`, RAEA_SOLARI_CELL_LIFE) read 1.34 against
  1.27. A 600-frame hold settles by about frame 300-450, but over a longer window the boiling reads higher, so the
  detector now fixes its window at 75 frames.
- **Offline, a 64-frame average of a frozen cache's frames reads 0.50**, the kitchen's level. Of a live cache's, it
  reads no better than as rendered. So what boils is slow: the light wanders over tens of frames, longer than
  MetalFX's history.

## Fireflies that linger, the frame's border, and dark squares (11-25, evening)

A baseline of every MALL RAT take at three frames (27 holds) had the frame-wide boiling under 1.5% on all but one,
and still failed most of them on a single worst square of 5 to 25%. Three different things were in that number.

- **A firefly that lingers.** A hot square's level is flat for 40 frames, jumps 30% in one frame, and takes about 30
  frames to fall back. One very bright sample (a bounce landing beside a small lamp) gets a large weight, temporal
  reuse keeps it, and spatial reuse spreads it to its neighbours. Whether a hold passed depended on whether one
  landed in its 75 frames: t7 187 read 0.88% (worst 10.1%) and, minutes later with nothing changed, 0.39% (worst
  2.0%). The party room (t5 1440), lit by small bulbs, has them all the time. This is the case RTXDI's boiling filter
  is for, and ReSTIR PT Enhanced (2026) describes the cause. raea's version is `SolariLighting.firefly_cap` (TSX
  `firefly`, RAEA_SOLARI_FIREFLY): after temporal reuse, a reservoir whose contribution is over N times the mean of
  its 8x8 block has its weight cut to that (restir.wesl, a `var<workgroup>` array and a barrier). On t5 1440:

  | firefly | boiling | worst square | frame's mean |
  |---|---|---|---|
  | off | 1.63% | 7.4% | 0 stops |
  | 32 | 1.42% | 4.9% | -0.003 |
  | 12 | 1.06% | 6.3% | -0.15 |
  | 6 | 0.73% | 3.0% | -0.29: STEADY |
  | 3 | 0.69% | 3.9% | -0.38 |

  It costs the light those samples carried, so it is off by default and a composition asks for it. Cutting the
  sample's confidence, not its weight (ReSTIR PT Enhanced's adaptive cap), lost nearly as much light (-0.22 at 6) and
  held less still (0.89%).
- **The frame's border boils more, in a band as wide as the spatial reuse radius.** By distance from the border, in
  8 px steps: 0.94, 0.79, 0.67, 0.62, 0.57, then 0.5 from 48 px in (t7 187). A border pixel has half a disc of
  neighbours to reuse from. Reflecting the neighbours into the frame helped the first few pixels (above); permuting
  the 3 px border strip that `permute_pixel` leaves unpermuted changed nothing (kept, it is harmless). What production
  renderers do is overscan: render a margin and crop it. raea has no overscan yet.
- **Dark squares read high as a plain ratio.** The detector divided each square's deviation by its level, so the
  darkest corners led (12% of nearly nothing). The eye's contrast threshold is constant in bright light (Weber) and
  rises as the square root of falling luminance in dim light (de Vries-Rose; Barten's model has both). kinds.rs now
  divides by sqrt(level x 0.05) below a level of 0.05 (linear; about 5 cd/m2 on a 100 cd/m2 display). t5 1440 read
  2.03% before and 1.59% after on the same frames.

What was looked up and already there: the world cache's lookups are jittered in the tangent plane (GI-1.0 and
world-space ReSTIR name hash-cell blotches and this cure), and its blend is the adaptive one (Boissé's, in Bevy 0.18).

For moving shots, ReVQ (No-Reference Rendered Video Quality Assessment, arXiv 2510.13349) is the design to follow:
windows of 5 frames warped onto their last with motion vectors, pixels without a correspondence masked, differences
over spans of 1 to 4 frames. A first try that chained 32 frames of motion vectors read 10% on a clean shot from the
warp's own error.

With the firefly filter at 6 on every take, 19 of 24 holds read STEADY (8 of 22 before). What was left, by cause:

- **Emitters thinner than a pixel flickered: the emitter history's clamp.** MALL RAT's ceiling tubes seen edge-on are
  1 to 3 px tall and changed shape every frame (t6 211 and t8 204: jitter 7.7% and 7.0% in their squares). With the
  history always taken (RAEA_EMIT_TAA_DEBUG=3) the jitter read 0.01%. emit_taa.wgsl held a still pixel's history
  inside the range of this frame's samples about it, so a lamp switched off goes dark at once; a tube that some of the
  jitter's positions miss altogether was cut to black on those frames. Eased (a quarter of the way to those bounds a
  frame) both holds read STEADY (jitter 2.2% and 1.2%), but lightbench's orb switched off then left +2.52 stops on its
  tile where it left +0.17 (RT32). Eased only where the history is thin (dark two pixels to both sides, across or
  down) the tubes read 3.6% and 2.1% and the orb +0.30, its rim. Two stricter tests of "thin" kept the rim out and
  lost the tubes (5.2 to 6.5%). So the engine's default is as it was (STILL_HOLD 1) and MALL RAT asks for it
  (`constants={{ emissiveClamp: { stillHold: 0.25 } }}`); the fix for every scene is a count, kept with the history,
  of the frames a pixel was missed. A softer gather kernel made it worse (11 to 13% at emissiveSharpness 3 and 1.5).
- **The border** (above): t3 1320's wall at the frame's right edge is lit in 8 px blocks that change each frame, and
  the corner squares of t5 900 and the court boil past the worst-square limit on some runs. Overscan is
  docs/raytracing.feature RT40, designed and not built.
- A hold near the limit passes on one run and fails on the next (t2 922 and t2 2304 failed in the baseline and read
  STEADY minutes later): what's left there is chance, a firefly landing in the 75 frames or not.

The moving detector (scratch moving3.py, ReVQ's design on raea's own motion vectors and depth) reads 0.5% a frame on
the court's door swing where the 32-frame chain read 10%, and tells one path a pixel from four (worst square 9.3%
against 5.8% over 4 frames). It did not tell `late` and emissive "taa" from the settings before them on that shot,
which has no sky edge or sign in it and whose light changes as the doors open. It needs a moving shot under steady
light to be judged on before it gates anything.


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

## What fixes the patches, measured (11-25, with raea-be)

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

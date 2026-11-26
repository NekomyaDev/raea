// Generated from raea-core-brief (core/brief) by its `declarations`; edit the Rust, not this.

export type Brief = { title: string, logline?: string, 
/**
 * who the work is for (a person or a client)
 */
for?: string, 
/**
 * YYYY-MM-DD
 */
due?: string, 
/**
 * who watches or plays it, and where
 */
audience?: string, 
/**
 * the one thing they should think afterwards
 */
believe?: string, 
/**
 * pitch, explain, story, play, tool: an open set, one guide each; a piece can mix two
 */
mode?: Array<string>, 
/**
 * the narrator and the tone
 */
voice?: string, sound?: string, 
/**
 * the project's committed world: palette by role, type, camera language, light, references
 */
look?: string, 
/**
 * the anti-goals
 */
notDoing?: Array<string>, 
/**
 * facts an agent wrote and the person hasn't confirmed, by key ("voice", "audience")
 */
assumed?: Array<string>, 
/**
 * prose sections beyond the fields above (Goal, Background), in order
 */
sections?: Array<Section>, deliver?: Array<Deliverable>, references?: Array<Reference>, agents?: Array<Agent>, 
/**
 * the project's stories, imported explicitly (raea/story, docs/story.md): one, or several by name. Kept as written:
 * raea reads nothing in it, the story SDK and the checks that import it do
 */
story?: unknown, 
/**
 * a scene, kit or shot made to a frozen plan and held to the review loop (docs/make.feature): `scene()`, `kit()`
 * or `shot()` from "raea/brief"; `raea make status` decides when it's done
 */
make?: Make, };

export type Section = { heading: string, text: string, };

export type Deliverable = { name: string, 
/**
 * "90 s", "16:9", "with sound"
 */
specs?: Array<string>, 
/**
 * the feed item it lands as
 */
item?: string, 
/**
 * its own brief module, when it differs from the project in mode, look or audience
 */
brief?: string, 
/**
 * its script module (storyboard or screenplay)
 */
script?: string, };

export type Reference = { target: string, note?: string, };

export type Agent = { name: string, role?: string, };

export type Script = { title: string, logline?: string, scenes: Array<Scene>, };

export type Scene = { 
/**
 * where and when ("INT. SENATE - NIGHT", or "The brief page")
 */
heading?: string, beats: Array<Beat>, 
/**
 * what the scene must land
 */
payoff?: string, 
/**
 * the story beat it plays, by its id (raea/story's compileScript writes it; docs/story.feature ST0c)
 */
beat?: string, };

export type Beat = { 
/**
 * what's seen (a screenplay's action)
 */
onScreen: string, 
/**
 * the move ("push in on the knife", "orbit left, keeps moving through the cut")
 */
camera?: string, 
/**
 * how this beat is cut into ("match on motion", "hard cut")
 */
cut?: string, lines?: Array<Line>, 
/**
 * sound and music cues ("strings drop out", "the theme's second hit")
 */
cues?: Array<string>, seconds?: number, 
/**
 * the word the beat cuts on, so a new take keeps the cut in sync with the transcript
 */
onWord?: string, };

export type Line = { 
/**
 * a character, or "narrator"
 */
who: string, says: string, 
/**
 * a screenplay's parenthetical ("quietly")
 */
how?: string, };

export type Taste = { title: string, 
/**
 * when this file is read ("Read for pitch, explain and story")
 */
note?: string, for?: string, 
/**
 * the kinds of work with their own taste file (motion, games, sound, ui)
 */
kinds?: Array<string>, sections: Array<TasteSection>, };

export type TasteSection = { 
/**
 * "Always", "Never", "Camera"
 */
heading: string, rules: Array<Rule>, };

export type Rule = { text: string, check?: RuleCheck, 
/**
 * what a named or a .ts check is held to (`{ band: 0.8 }`); a form carries its own value
 */
values?: Record<string, number | string | boolean>, 
/**
 * how bad what its check finds is: error, warn or note (unset: a form's findings are errors, another check's keep
 * its own level)
 */
level?: RuleLevel, };

export type RuleCheck = Check | string;

export type RuleLevel = "error" | "warn" | "note";

export type Check = { "form": "text-hold", "value": number } | { "form": "text-above", "value": number } | { "form": "text-below", "value": number } | { "form": "text-margin", "value": number } | { "form": "loudness", "value": number } | { "form": "still-max", "value": number };

export type Parts = { concept: Concept, 
/**
 * by name; the build's parts are typed from these keys, so a planned part the build skips is a tsc error
 */
parts: { [key in string]: Part }, };

export type Concept = { 
/**
 * what it is ("flight feather", "toy rocking horse")
 */
what: string, style: string, 
/**
 * its largest extent, in metres
 */
size: number, 
/**
 * how close it's seen, in metres: sets how much detail up close must hold
 */
seenFrom: number, material?: string, use?: string, };

export type Level = "primary" | "secondary" | "tertiary";

export type Count = number | { min: number, max: number, };

export type Part = { 
/**
 * the label it carries into the world ("seat", "leaf", "gem")
 */
role: string, level: Level, 
/**
 * the part it sits on or in; none for the root
 */
parent?: string, count?: Count, 
/**
 * its own, else its parent's, else the concept's
 */
material?: string, style?: string, 
/**
 * what a near crop of it must show
 */
near?: string, };

export type Make = { kind: MakeKind, 
/**
 * the pictures the job answers to (paths beside the brief): what is in it and where for a scene, the style for a kit
 */
references?: Array<string>, 
/**
 * one paragraph every reference prompt carries: era, palette, materials, wear, and one fictional maker
 */
style?: string, 
/**
 * by name, from the kind's set (DIALS); one left out is read from the references
 */
dials?: { [key in string]: MakeDial }, objects?: Array<MakeObject>, shots?: Array<MakeShot>, 
/**
 * the compositions the job's checks hold (relative to the brief): each needs a clean render or check newer than it
 */
compositions?: Array<string>, 
/**
 * what must exist when it's done, relative to the brief; replaces the kind's own list (`deliverables`)
 */
deliverables?: Array<string>, 
/**
 * the most critic rounds the job gets; at the cap it is delivered with what's still missing (8 by default)
 */
rounds?: number, 
/**
 * a cut job's contract: the brief as rules (cut.rs)
 */
cut?: MakeCut, 
/**
 * a goal job's contract: the goal and its rules (goal.rs)
 */
goal?: MakeGoal, 
/**
 * what the job shows but never delivers, relative to the brief (pictures, render tests, examples): each a file or a
 * `dir/` (review/ and previews/ when left out). Deliverables are what it hands over; previews are how it got there
 */
previews?: Array<string>, };

export type MakeKind = "scene" | "kit" | "shot" | "cut" | "goal";

export type MakeSource = "customer" | "reference";

export type MakeDialValue = number | string;

export type MakeDial = MakeDialValue | { value: MakeDialValue, source?: MakeSource, 
/**
 * what the value meant for this job, in one line
 */
means?: string, };

export type MakeRoute = "solid" | "rule" | "gen" | "library" | "picture";

export type MakeOrigin = "reference" | "invented";

export type MakePick = { 
/**
 * "tripo-h31", "meshy", ...
 */
generator: string, 
/**
 * one line: why this one ("sharper dial numerals; Tripo fused the handset to the cradle")
 */
why: string, };

export type MakeObject = { 
/**
 * snake_case, unique in the job
 */
id: string, label: string, 
/**
 * real height in metres
 */
h?: number, 
/**
 * [width, depth, height] in metres, for boxy things (generators invent depth)
 */
dims?: [number, number, number], 
/**
 * largest extent in metres, for things that lie flat (tools, papers)
 */
max?: number, route?: MakeRoute, 
/**
 * one object, its condition and its materials; never a real brand
 */
prompt?: string, source?: MakeOrigin, 
/**
 * where it is in reference 0: [x0, y0, x1, y1] in 0..1
 */
crop?: [number, number, number, number], 
/**
 * where it stands in a scene: metres, the floor's front-left corner at the origin, rot 0 = facing the entrance
 */
at?: [number, number, number], rot?: number, hero?: boolean, 
/**
 * how large it is planned to be seen, which is how much of it gets built: 0 the hero (half the frame or more),
 * 1 featured (under 50%), 2 dressing (under 20%), 3 background (under 8%: a grey box reads), 4 far (under 2%: a
 * silhouette). A render that shows it finer than this says so (`lod-closer`; `raea scene lod` lists every part)
 */
lod?: number, pick?: MakePick, 
/**
 * what was fixed by script ("glass made transmissive, label kept opaque")
 */
fixes?: Array<string>, };

export type MakeShot = { 
/**
 * [a-z0-9_]: the file it lands as (out/shots/<name>.png or .mp4)
 */
name: string, 
/**
 * in a crew's words: "slow push along the counter, rack focus to the register"
 */
description?: string, 
/**
 * static, dolly, truck, pedestal, crane, pan, tilt, orbit, dollyZoom, path
 */
move?: string, seconds?: number, lens?: number, 
/**
 * none, subtle, walk
 */
handheld?: string, 
/**
 * a still rather than a move
 */
still?: boolean, 
/**
 * the composition that renders it (relative to the brief): `raea make framing` and `lenschart` draw it
 */
composition?: string, 
/**
 * its frame rate, for the frames `seconds` spans (30 when left out)
 */
fps?: number, };

export type MakeCut = { 
/**
 * the brief as text, relative to brief.ts (`pdftotext -layout brief.pdf media/brief.txt`): every `from` is found in it
 */
brief: string, 
/**
 * how many cuts the brief asks for; fewer planned is held open until an answered `open` item about "count" says why
 */
count?: MakeQuoted<number>, 
/**
 * a cut's length when its own `seconds` says none: [least, most]
 */
seconds?: [number, number], 
/**
 * the formats every cut is delivered in when its own `formats` says none: "9:16", "16:9", "1:1", "4:5"
 */
formats?: Array<string>, cuts?: Array<MakeCutPlan>, 
/**
 * what is said or shown: in every cut unless `every: false` (then in one or more)
 */
say?: Array<MakePhrase>, 
/**
 * what no cut says or shows: a banned claim, a word, a name
 */
never?: Array<MakePhrase>, 
/**
 * the line every cut ends on, heard or on screen in its last `within` seconds
 */
cta?: MakeCta, 
/**
 * the brief's open items ([TO CONFIRM]): each holds the job open until it has an answer
 */
open?: Array<MakeOpen>, 
/**
 * every face whole and every text big enough, by the frames check of each file (true when left out)
 */
frames?: boolean, };

export type MakeQuoted<T> = { value: T, 
/**
 * the brief's own words, as they are in its text
 */
from: string, };

export type MakeCutPlan = { 
/**
 * [a-z0-9_], unique: each format lands as out/cuts/<id>_<W>x<H>.mp4 ("a1_s1_9x16.mp4")
 */
id: string, angle?: string, 
/**
 * solo or mashup
 */
kind?: string, 
/**
 * one of the brief's hook types ("Direct Problem", "Reaction")
 */
hook?: string, 
/**
 * captions, hook (a text hook) or none
 */
text?: string, formats?: Array<string>, seconds?: [number, number], };

export type MakePhrase = { 
/**
 * as it's said or shown; compared in lower case, by letters and digits ("24/7" and "24-7" are the same)
 */
text: string, from: string, 
/**
 * `say` only: false when one cut saying it is enough
 */
every?: boolean, };

export type MakeCta = { text: string, from: string, 
/**
 * the other lines a brief rotates (two host lines, a paid take's): any one lands it, each found in the brief
 */
also?: Array<string>, 
/**
 * how near the end it must land, in seconds (8 when left out)
 */
within?: number, };

export type MakeOpen = { question: string, from: string, 
/**
 * what it decides: "count", "cta", "offer", a cut's id...
 */
about?: string, 
/**
 * the person's answer, as they gave it, and who gave it: until then the job can't be done
 */
answer?: string, by?: string, };

export type MakeGoal = { 
/**
 * the goal as the person said it
 */
goal: string, 
/**
 * what must hold before the agent may stop: every one, each checked in code or by a fresh critic
 */
rules?: Array<MakeGoalRule>, };

export type MakeGoalRule = { 
/**
 * a short name: [a-z0-9_], how status, the critic and NOTES.md name it
 */
id: string, 
/**
 * a command that must exit 0, run in the brief's folder (`bun test`, `raea check edit.tsx`)
 */
run?: string, 
/**
 * the seconds `run` gets before it counts as failed (300 when left out)
 */
timeout?: number, 
/**
 * a file that must exist, relative to the brief (`a|b`: either; `dir/`: three or more pictures in it)
 */
file?: string, 
/**
 * words that must be true of the work, judged by a fresh critic (`raea make judge`), never by the agent that did it
 */
says?: string, };

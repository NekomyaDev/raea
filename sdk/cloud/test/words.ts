// raea words (server/words.ts) offline: a word cut short is an error, words after a cut missing, the same words pass,
// a different word a warning, a crossfade dip inside a word found, a word on the first frame flagged only when the
// sound is already at speech level there, and two words handed back as one split. Run: node test/words.ts
import { compare, fades, norm, split } from "../server/words.ts"
const w = (t: string, s: number, e: number) => ({ text: t, start: s, end: e })
const ok = (c: boolean, m: string) => { if (!c) { console.log("not ok", m); process.exitCode = 1 } else console.log("ok", m) }
ok(norm("$1,000.") === "1000", "norm money")
const heard = [w("So", 0.1, 0.3), w("he", 0.3, 0.4), w("hurt", 0.4, 0.6), w("his", 0.6, 0.7), w("paw,", 0.7, 0.9), w("and", 1.0, 1.1), w("some", 1.1, 1.3), w("test", 1.3, 1.5)]
const f = compare("So he hurt his paw and some tests. Because it's expensive", heard)
ok(f.some((x) => x.kind === "clipped" && x.says.includes("tests")), "tests cut short to test is an error")
ok(f.filter((x) => x.kind === "missing").length === 3, "the three words after the cut are missing")
ok(compare("So he hurt his paw", heard.slice(0, 5)).length === 0, "the same words pass")
ok(compare("So he hurt his dog", heard.slice(0, 5)).some((x) => x.kind === "changed"), "a different word is a warning")
// levels: speech at -20 dB, a 300 ms dip to -40 inside "hurt"
const db = Array.from({ length: 200 }, (_, i) => (i >= 40 && i < 70 ? -40 : -20))
const hw = [w("So", 0.1, 0.3), w("hurt", 0.35, 0.8), w("his", 0.8, 1.2), w("paw", 1.2, 1.9)]
ok(fades(db, hw, 2).some((x) => x.kind === "faded"), "a crossfade dip inside a word is found")
ok(!fades(Array(200).fill(-20), hw, 2).some((x) => x.kind === "faded"), "even speech has no dip")
ok(fades(Array(200).fill(-20), [w("Hi", 0.0, 0.3), ...hw.slice(1)], 2).some((x) => x.kind === "edge"), "a word on the first frame is flagged when the sound is loud there")
ok(!fades([-60, -60, -60, ...Array(197).fill(-20)], [w("Hi", 0.0, 0.3), ...hw.slice(1)], 2).some((x) => x.kind === "edge"), "Whisper putting a word at 0 over silence is not")
ok(split([w("for $60", 1, 2)]).map((x) => x.text).join("|") === "for|$60", "two words handed back as one are split")

# Camera (Map/config.ifo) - JMXVCAMR1002 (obsolete)

A single saved editor viewpoint. Upstream marks the format obsolete, and the
v1.188 client never opens the file — see the verdict at the end.

The layout below is **openroad's own**, decoded byte by byte from the only
corpus sample and forced by internal geometric consistency. Upstream reference
(a raw annotated hex dump, superseded by the table below):
`SilkroadDoc.wiki/JMXVCAMR1002` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVCAMR1002

## Decoded layout (openroad, 2026-08-12)

`Map/config.ifo` is the only CAMR in the corpus: 111 bytes, EOF-exact. The raw
dump above is now fully named — every field below is forced by internal
geometric consistency, not guessed:

| Off | Type | Field | Value in this file |
|--|--|--|--|
| 0x00 | char[12] | signature | `JMXVCAMR1002` |
| 0x0c | u16 | **regionID** | 27581 → x=189, z=107 (`Map/107/189.m` exists) |
| 0x0e | u32 | UNKNOWN | 6 |
| 0x12 | f32[3] | **eye** | (650.14, 1273.50, 1881.89) region-local |
| 0x1e | f32[3] | **at** | (678.67, 141.00, 1140.41) |
| 0x2a | f32[3] | **up** (unit) | (0.03216, 0.54805, −0.83583) |
| 0x36 | f32[3] | **forward** (unit) | (0.02107, −0.83644, −0.54765) |
| 0x42 | f32 | **pitch** | 0.9907623 rad = 56.766° |
| 0x46 | f32 | **yaw** | −3.1800485 rad |
| 0x4a | f32 | **roll** | 0.0 |
| 0x4e | f32 | **distance** | 1353.9409 |
| 0x52 | u8 / u8 / u16 | UNKNOWN ×3 | 1 / 237 / 18 |
| 0x56 | f32 | nearPlane | **1.0** |
| 0x5a | f32 | farPlane | **5500.0** |
| 0x5e | f32 | fov | **45.0°** |
| 0x62 | f32 | UNKNOWN (== at.y) | 141.0 |
| 0x66 | u8[5] | UNKNOWN bools | 1,1,1,0,1 |
| 0x6b | f32 | UNKNOWN | 0.502 → 0x6b+4 = 111 = filesize |

Proofs: `|at − eye|` = 1353.94094 vs stored `distance` 1353.94092 (Δ 2.6e−5) ·
`eye == at − forward·distance` (max err 2.9e−5) ·
`forward == (cos p·sin y, −sin p, cos p·cos y)` (err ≤ 8.7e−8) ·
`up == (sin p·sin y, cos p, sin p·cos y)` (err ≤ 3.5e−8) · `|up| = |forward| = 1`,
`up·forward = 9.1e−8`, and `right = up × forward` is unit with y = −1.4e−8 ⇒ zero
roll. The basis is **left-handed (D3D)**, matching `D3DXMatrixLookAtLH`.

**Our client hardcoded all three projection constants and all three disagreed**
(`client/src/plugins/camera.rs:104-106`): near 0.500 vs **1.0**, far 200000.0 vs
**5500.0**, fov 1.0 rad (≈57.3°) vs **45°**. See EP-26 / #108.

**What #108 adopted, and what it did not.** `near` and `fov` are taken verbatim.
`far` is **not**: our fog fades from `VISIBLE_RANGE * REGION_SIZE` (3840) to
`(VISIBLE_RANGE + FOG_RANGE) * REGION_SIZE` (5760), and Bevy's linear fog is
`alpha = (d − start) / (end − start)`, so at 5500 terrain is only ~86 % opaque —
a 5500 far plane clips partially transparent geometry out of the outer fog ring
instead of letting it finish fading into the horizon-matched `FOG_COLOR`. The far
plane is now derived from those streaming constants, so retuning them cannot
reintroduce the clip.

Our fog ring width is itself ungrounded (`VISIBLE_RANGE`/`FOG_RANGE` were
promoted from bare literals with no citation), so 5760 and 5500 are two
independently arbitrary numbers that happen to sit 4.7 % apart. Aligning the
streaming radius to 5500 is a defensible follow-up, but it touches six files and
is not a camera-constants change.

Two cautions on this record. `fov` is stored without an aspect ratio, so
"vertical" is inferred from the left-handed D3D basis
(`D3DXMatrixPerspectiveFovLH` takes a y-direction FOV) and from 45° matching
Bevy's own default — **likely, not verified**. And the upstream source marks this
format obsolete ("this file is not used anymore"), with `n = 1` in every build we
have: the `[V]` confidence covers the *byte layout*, not the claim that this
projection triple is what the shipped gameplay camera used.

> **Round 2 (F3) — the `n = 1` blocker is terminal, so the residual unknowns are moot.** The only
> known `JMXVCAMR1002` sample is `Map/config.ifo`, and an exhaustive grep of the v1.188 client
> string table for `.ifo`/`.mfo` literals shows **`config.ifo` never appears** — the client does
> not open it. So there is no second sample to come and no consumer whose behaviour could name the
> remaining fields. **CAMR is `do-not-wire`**: close the open items rather than leaving them
> waiting for a corpus that cannot grow. (The three projection constants already adopted under
> #108 stand on their own — they were adopted for being *sourced values* against three unsourced
> hardcodes, not for being the shipped camera's.) It is a single saved
editor viewpoint — which is also why only the projection transfers and the pose
does not (`distance` 1353.94 is far outside our 40–400 clamp; `pitch` 0.9908 rad
*is* inside `[0.1, 1.45]`, so `distance` alone is what disqualifies the pose).

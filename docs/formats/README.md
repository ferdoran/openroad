# Format details

One file per SRO data format: what the bytes mean, as field tables.

## What these are, and where they come from

Each file gives a format's **field layout** — offset, size, type, name — plus
whatever openroad has verified about it from the user's own PK2 corpus. Field
layouts are facts about a file format, and that is deliberately all these
documents carry.

Most files state a layout **derived from openroad's own parser** in
`client/src/assets/` (or `bevy_pk2/` for the archive container itself), verified
against corpus probes. Where our parser and an external source disagree, our
parser wins and the discrepancy is noted — several formats here correct the
commonly circulated description (`.cpd` and `.dof` `Type` are an `i16` pair, not
a `u32`; `.bmt` uses six flag bits, not thirty-two; `.ddj`'s size field is
unreliable).

A few formats have **no openroad parser yet**. Those files say so at the top and
mark their layout unverified — treat them as a lead, not a fact. Pages whose whole
content would have been a restatement of an unlicensed upstream description, with
nothing of our own added, are not kept here at all.

Every file names the external reference it was cross-checked against. Where that
is the [SilkroadDoc wiki](https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki),
the citation is a pointer for the reader, not a statement that the text came from
there: that wiki carries no licence, so this project takes **facts and field
layouts** from it and does not copy its prose, pseudocode or ImHex patterns.
The same rule governs every other unlicensed source — see
[`../../CONTRIBUTING.md`](../../CONTRIBUTING.md) for what each licence class permits
and for the material this repository deliberately does not publish.

## Formats

- `pk2-jmxpack.md`
- `nvm-jmxvnvm.md`
- `dof-jmxvdof.md`
- `bsr-jmxvres.md`
- `cpd-jmxvcpd.md`
- `bms-jmxvbms.md`
- `bmt-jmxvbmt.md`
- `ddj-jmxvddj.md`
- `mapo-jmxvmapo.md`
- `mapm-jmxvmapm.md`
- `mapt-jmxvmapt.md`
- `mfo-jmxvmfo.md`
- `envi-jmxvenvi.md`
- `obji-jmxvobji.md`
- `2dti-jmxv2dti.md`
- `bsk-jmxvbsk.md`
- `ban-jmxvban.md`
- `efp-jmxveff.md`
- `camr-jmxvcamr.md`
- `imgfont-jmxvimg.md` (not from the wiki: the three `Media/fonts/*.dat` glyph bitmaps — a test triple, not a font)
- `ainavdata.md`
- `newinterface-2dt.md` (+ openroad's corpus notes: 976-byte entries, CP949, style flags, absolute coords)
- `newinterface-type.md`
- `sv-t.md`
- `divisioninfo.md`
- `gateport.md`
- `sroptionset.md`
- `resinfo.md` (the classic UI grammar itself — read blocks by key name, not by line offset)
- `textdata-characterdata.md`
- `textdata-itemdata.md` (itemdata columns + magicoption.txt, verified against v1.188)
- `textdata-skilldata.md` (skilldata columns + fourcc parameter stream, verified against v1.188)
- `textdata-skilleffect.md` (skillaniset2 + skilleffectset columns, verified against v1.188)
- `shops.md` (NPC shop inventory: refshopgroup chain, denormalized)
- `minimap.md` (in-game minimap data: Media.pk2 tiles, dots, area names)
- `worldmap.md` (M-key world map: worldmap_mapinfo/localinfo tables)
- `textdata-regioninfo-effectenvsnd.md` (zone sound tables: sectors → zone → BGM + day/night ambience)
- `textdata-effectsound.md` (SFX registry: `(object, handle, skill_ID, event1..3)` → `.wav` + per-row volume)

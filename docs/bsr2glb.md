# bsr2glb — .bsr → .glb / .fbx converter

Converts a Silkroad `.bsr` resource — including its `.bms` meshes, `.bmt`
materials, `.ddj` textures, `.bsk` skeleton and `.ban` animations — into a
single self-contained binary glTF (`.glb`) and/or binary FBX 7.4 (`.fbx`)
file, viewable in Blender or any glTF/FBX-capable tool. It reuses the
client crate's format parsers through the client's parser-only lib target
(`client/src/lib.rs`), so there is exactly one implementation of each byte
layout in the repo.

## Usage

```bash
# single resource, PK2 resolved via SRO_PK2_PATH -> SRO_PATH -> ./assets
make bsr2glb BSR='res\char\china\chinaman_fighter.bsr' OUT=fighter.glb

# FBX instead (inferred from the extension, or force with FORMAT=)
make bsr2glb BSR='res\char\china\chinaman_fighter.bsr' OUT=fighter.fbx
make bsr2glb BSR='res\char\china\chinaman_fighter.bsr' FORMAT=both OUT=fighter

# batch: every .bsr under a prefix, mirrored into an output directory
make bsr2glb PREFIX='res\item\china\weapon' OUT=weapons/ FORMAT=both

# direct invocation (either slash style works, paths are case-insensitive)
cargo run -p tools --bin bsr2glb -- --bsr 'res/char/china/chinaman_fighter.bsr' \
    [--pk2 /path/to/Data.pk2 | --dir /path/to/extracted-tree] \
    [--out x.glb] [--format glb|fbx|both] [--raw]
```

- `--pk2` defaults to `$SRO_PK2_PATH`/`$SRO_PATH`/`assets` + `/Data.pk2`.
- `--dir` reads from an extracted directory tree (e.g. `make pk2 unpack` output)
  instead of a PK2.
- `--format` defaults to whatever the `--out` extension implies, else glb.
- Batch mode (`--prefix`) logs per-file errors and keeps going; the exit code
  is only non-zero when nothing converted. Default output directory:
  `out_models/`.

## Coordinate convention

By default the export bakes the same conversion the client applies at spawn
time (`scale.x = -1` placement + reversed triangle winding, see
`client/src/util/mesh.rs`): X is negated on positions/normals/translations,
rotations are conjugated across the YZ plane, winding is flipped, and the
inverse bind matrices are computed from the mirrored bone transforms. The
result looks exactly like in-game. `--raw` (`RAW=1`) skips all of that and
exports the data as authored, which appears mirrored compared to in-game.

## Mapping

| SRO | glTF |
| --- | --- |
| `.bsr` primitive groups | one node per group under the resource root, one child node per mesh |
| `.bms` mesh | mesh with POSITION/NORMAL/TEXCOORD_0 (+TEXCOORD_1 for lightmap UVs), u16 indices |
| `.bms` bone weights (2 influences, u16 fractions) | JOINTS_0/WEIGHTS_0 vec4, remapped from the mesh-local bone-name list to skin joints, renormalized to sum 1 |
| `.bsk` bones (incl. dummies, file order) | joint node hierarchy (TRS = parent transform); inverse bind matrices from the accumulated parent chain |
| `.bmt` material | `baseColorTexture` from the diffuse `.ddj` (white factor; untextured materials use the BMT ambient color), flag 0x1 → `doubleSided`, flag 0x8 → emissive texture/factor, flag 0x200 → `alphaMode: MASK` (cutoff 0.5) unless the resource marks alpha as sheen |
| `.ddj` texture | embedded PNG (DXT1/3/5 via the `image` crate, uncompressed D3D formats via the client's decoders) |
| `.ban` animation | one glTF animation, LINEAR translation+rotation channels per bone, times in seconds with min/max; named `{group}/{type_id}:{ban name}` via the BSR's animation groups |

## FBX specifics

The `.fbx` output is binary FBX 7.4 (the low-level encoding comes from the
`fbxcel` crate; the object graph is built in `tools/src/bin/bsr2glb/fbx.rs`).
Scene mapping mirrors the glTF one: bones are `LimbNode` models (Blender
builds an armature from them), each skinned geometry gets a `Skin` deformer
with one `Cluster` per used bone (`Transform` = inverse bind matrix,
`TransformLink` = global bind), textures are embedded PNG `Video` content,
and every `.ban` becomes an `AnimationStack`. Points to know:

- **Units**: `UnitScaleFactor = 100` (SRO units are meters, FBX counts cm).
- **Rotations**: FBX animation curves are XYZ-order euler *degrees*; each
  keyframe quaternion is converted with per-channel ±360° unwrapping for
  continuity. Interpolation between sparse keys is linear in euler space,
  not slerp — animations crossing gimbal regions can wobble slightly (the
  glb export does not have this limitation).
- **Loop mode**: cyclic animations get a ` [cyclic]` suffix on the stack
  name (FBX has no loop flag either).
- FBX files are considerably larger than the glb (every animation curve
  carries its own 64-bit key-time array); the big arrays are
  zlib-compressed, which every importer supports.

## Known limitations

- **Sheen alpha is exported opaque**: resources with an EnvMap mod (weapons,
  metal armor) use their texture alpha as a metallic/sheen mask; exporting it
  as transparency would punch holes, so those materials are `OPAQUE` and the
  mask is only present in the PNG's alpha channel.
- **Lightmaps**: `TEXCOORD_1` is exported when present, but the lightmap
  `.ddj` itself is not bound (glTF core has no lightmap slot).
- **Loop semantics**: glTF has no loop flag; each animation carries
  `extras: {"sro_animation_type": "Cyclic"|"OneShot", "sro_fps": n}`.
- Particle/effect mods, collision meshes, and the attachment slot table are
  not exported.
- Phantom bones (mesh bone names missing from the skeleton, e.g. EU leg
  armor `Bone03`) become zero-weight influences with a warning — matching
  the runtime's static-fallback behavior.
- Exotic DDS formats (DXT2/4, DX10 headers) are skipped with a warning; the
  material stays untextured.

## Verification

`cargo test -p tools --bin bsr2glb` runs the GLB packing/remap and FBX
euler/encoding unit tests. With `SRO_PK2_PATH` set, the
`converts_real_bsr_files_to_valid_glb` test additionally converts one
character and one building resource from the real `Data.pk2` to both
formats, re-imports the GLB with the `gltf` crate (accessor bounds,
IBM/joint counts, sampler min/max) and re-parses the FBX with `fbxcel`
(object graph + connection ids).

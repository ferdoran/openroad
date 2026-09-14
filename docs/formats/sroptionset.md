# SROptionSet.dat

Source: `SilkroadDoc.wiki/SROptionSet.md` — https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/SROptionSet

The original client's saved in-game options (video/audio/controls/gameplay). It
is a **client-root runtime file, not a PK2 entry**. openroad reads it (import
only) but never writes it — our own options persist to `user_settings.yaml`.

Parser: `client/src/plugins/settings/sroptionset.rs` (raw TLV) →
`client/src/plugins/settings/options.rs` (`GameOptions` semantic model).

## Header (9 bytes, little-endian)

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `Unknown0` | opaque — version or flags, not interpreted |
| 1 | u8 | `Unknown1` | opaque |
| 4 | u32 LE | `Unknown2` | opaque |

## Record stream

Records repeat to end of file:

| size | type | field | notes |
|---|---|---|---|
| 2 | u16 LE | `Id` | selects the value width — see the table below |
| 2 | u16 LE | `Unknown0` | always 0; **not** a name length or skip count |
| varies | — | `Value` | width is a function of `Id` alone |

The stream is **not self-describing**: the value width is a function of `id`
only. An unknown `id` therefore makes the remainder unparseable, so the parser
stops and returns the records decoded so far. Every read is length-guarded — a
truncated value ends parsing gracefully. No read ever panics on file input.

## Resolved id → width

| id range          | width | type | group / meaning                                   |
|-------------------|-------|------|---------------------------------------------------|
| `1..=15`          | 2     | u16  | Graphic 1 quality sliders                         |
| `101..=115`       | 2     | u16  | Graphic 2 quality sliders                         |
| `501`, `601`      | 1     | u8   | Type (Graphic 1 / 2)                              |
| `502`, `602`      | 1     | u8   | Brightness (Graphic 1 / 2)                        |
| `503`, `603`      | 4     | u32  | WindowResolutionWidth (Graphic 1 / 2)            |
| `504`, `604`      | 4     | u32  | WindowResolutionHeight (Graphic 1 / 2)           |
| `1001..=1003`     | 4     | u32  | BGM / FX / Environmental volume                   |
| `1004..=1006`     | 1     | bool | BGM / FX / Environmental on-off                   |
| `2001..=2028`     | 1     | bool | Setting-tab toggles (`2015` = isWindowMode → Video) |
| `3001..=3099`     | 4     | u32  | KeyMap custom shortcuts (Win32 VK codes, stored raw) |
| `3101`            | 1     | bool | isMouseShortcutSwapped                             |
| anything else     | —     | —    | **UNKNOWN → stop parsing**                         |

Notes / UNKNOWNs:
- The header fields are opaque; their exact meaning is UNKNOWN.
- The `Type` / `Brightness` value scales and the volume slider range are
  UNKNOWN. openroad's `GameOptions::default()` is an openroad baseline, not a
  reverse-engineered original default (we never have a real `.dat` here).
- KeyMap VK→Bevy `KeyCode` translation is deferred to the UI layer.

# OptionSet.csv
|ID  |OptionTab|OptionScope          |Name                            |ValueType     |
|----|---------|---------------------|--------------------------------|--------------|
|1   |Video    |Graphic 1            |ShadowDetail                    |2 bytes       |
|2   |Video    |Graphic 1            |BackgroundSightRange            |2 bytes       |
|3   |Video    |Graphic 1            |CharacterSightRange             |2 bytes       |
|4   |Video    |Graphic 1            |WaterReflection                 |2 bytes       |
|5   |Video    |Graphic 1            |WaterDetail                     |2 bytes       |
|6   |Video    |Graphic 1            |MetallicSheen                   |2 bytes       |
|7   |Video    |Graphic 1            |LightEffect                     |2 bytes       |
|8   |Video    |Graphic 1            |TextureFiltering                |2 bytes       |
|9   |Video    |Graphic 1            |TextureDetail                   |2 bytes       |
|10  |Video    |Graphic 1            |LensFlare                       |2 bytes       |
|11  |Video    |Graphic 1            |BloomEffect                     |2 bytes       |
|12  |Video    |Graphic 1            |DynamicAnimation                |2 bytes       |
|13  |Video    |Graphic 1            |EffectQuallity                  |2 bytes       |
|14  |Video    |Graphic 1            |                                |2 bytes       |
|15  |Video    |Graphic 1            |                                |2 bytes       |
|101 |Video    |Graphic 2            |ShadowDetail                    |2 bytes       |
|102 |Video    |Graphic 2            |BackgroundSightRange            |2 bytes       |
|103 |Video    |Graphic 2            |CharacterSightRange             |2 bytes       |
|104 |Video    |Graphic 2            |WaterReflection                 |2 bytes       |
|105 |Video    |Graphic 2            |WaterDetail                     |2 bytes       |
|106 |Video    |Graphic 2            |MetallicSheen                   |2 bytes       |
|107 |Video    |Graphic 2            |LightEffect                     |2 bytes       |
|108 |Video    |Graphic 2            |TextureFiltering                |2 bytes       |
|109 |Video    |Graphic 2            |TextureDetail                   |2 bytes       |
|110 |Video    |Graphic 2            |LensFlare                       |2 bytes       |
|111 |Video    |Graphic 2            |BloomEffect                     |2 bytes       |
|112 |Video    |Graphic 2            |DynamicAnimation                |2 bytes       |
|113 |Video    |Graphic 2            |EffectQuallity                  |2 bytes       |
|114 |Video    |Graphic 2            |                                |2 bytes       |
|115 |Video    |Graphic 2            |                                |2 bytes       |
|501 |Video    |Graphic 1            |Type                            |u8            |
|502 |Video    |Graphic 1            |Brightness                      |u8            |
|503 |Video    |Graphic 1            |WindowResolutionWidth           |u32           |
|504 |Video    |Graphic 1            |WindowResolutionHeight          |u32           |
|601 |Video    |Graphic 2            |Type                            |u8            |
|602 |Video    |Graphic 2            |Brightness                      |u8            |
|603 |Video    |Graphic 2            |WindowResolutionWidth           |u32           |
|604 |Video    |Graphic 2            |WindowResolutionHeight          |u32           |
|1001|Audio    |BackgroundVolume     |BackgroundVolumeSlider          |u32           |
|1002|Audio    |FXVolume             |FXVolumeSlider                  |u32           |
|1003|Audio    |EnvironmentalVolume  |EnvironmentalVolumeSlider       |u32           |
|1004|Audio    |BackgroundVolume     |BackgroundVolumeCheckbox        |bool          |
|1005|Audio    |FXVolume             |FXVolumeCheckBox                |bool          |
|1006|Audio    |EnvironmentalVolume  |EnvironmentalVolumeCheckbox     |bool          |
|2001|Setting  |Community            |GameGuideCheckBox               |bool          |
|2002|Setting  |Community            |PartyInvitationCheckbox         |bool          |
|2003|Setting  |Community            |ExchangeRequestCheckbox         |bool          |
|2004|Setting  |Community            |PersonalMsgCheckbox             |bool          |
|2005|Setting  |                     |                                |bool          |
|2006|Setting  |                     |                                |bool          |
|2007|Setting  |                     |                                |bool          |
|2008|Setting  |Display              |SystemUIAutoHideCheckbox        |bool          |
|2009|Setting  |Display              |QuickPartyViewBuffStatusCheckbox|bool          |
|2010|Setting  |Indicate Name Setting|OwnNameCheckbox                 |bool          |
|2011|Setting  |Indicate Name Setting|OtherNameCheckbox               |bool          |
|2012|Setting  |Indicate Name Setting|MonsterNamCheckbox              |bool          |
|2013|Setting  |Indicate Name Setting|NPCNameCheckbox                 |bool          |
|2014|Setting  |Indicate Name Setting|GuildNameCheckbox               |bool          |
|2015|Video    |Window Mode          |isWindowMode                    |bool          |
|2016|Setting  |Others               |HighSettingIntroCheckbox        |bool          |
|2017|Setting  |Display              |FortressWarMarkCheckbox         |bool          |
|2018|Setting  |Display              |HideAbilityPetsCheckbox         |bool          |
|2019|Setting  |Others               |HPWarningCheckbox               |bool          |
|2020|Setting  |Others               |MPWarningCheckbox               |bool          |
|2021|Setting  |Display              |SelfConditionCheckbox           |bool          |
|2022|Setting  |Display              |COSConditionCheckbox            |bool          |
|2023|Setting  |Display              |PartyMemberStatusCheckbox       |bool          |
|2024|Setting  |Display              |MonsterConditionCheckbox        |bool          |
|2025|Setting  |Display              |OpenTheGuideCheckbox            |bool          |
|2026|Setting  |Display              |ActivateActionShortcutCheckbox  |bool          |
|2027|Setting  |Others               |CameraRotationMethod1Checkbox   |bool          |
|2028|Setting  |Others               |CameraRotationMethod2Checkbox   |bool          |
|3001|KeyMap   |Custom shortcut      |KeyCharacter                    |u32 (KeyCode) |
|3002|KeyMap   |Custom shortcut      |KeyInventory                    |u32 (KeyCode) |
|3003|KeyMap   |Custom shortcut      |KeySkill                        |u32 (KeyCode) |
|3004|KeyMap   |Custom shortcut      |KeyAction                       |u32 (KeyCode) |
|3005|KeyMap   |Custom shortcut      |KeyParty                        |u32 (KeyCode) |
|3006|KeyMap   |Custom shortcut      |KeyQuest                        |u32 (KeyCode) |
|3007|KeyMap   |Custom shortcut      |KeyCommunity                    |u32 (KeyCode) |
|3008|KeyMap   |Custom shortcut      |KeyWorldMap                     |u32 (KeyCode) |
|3009|KeyMap   |Custom shortcut      |KeyBerserkerMode                |u32 (KeyCode) |
|3011|KeyMap   |Custom shortcut      |KeyHelp                         |u32 (KeyCode) |
|3012|KeyMap   |Custom shortcut      |KeyViewDropItem                 |u32 (KeyCode) |
|3013|KeyMap   |Custom shortcut      |KeyMouseQuickSlot               |u32 (KeyCode) |
|3014|KeyMap   |Custom shortcut      |KeySitStand                     |u32 (KeyCode) |
|3015|KeyMap   |Custom shortcut      |KeyAutoPickup                   |u32 (KeyCode) |
|3016|KeyMap   |Custom shortcut      |KeyCOSInfo                      |u32 (KeyCode) |
|3017|KeyMap   |Custom shortcut      |KeyCOSRide                      |u32 (KeyCode) |
|3018|KeyMap   |Custom shortcut      |KeyCOSRelease                   |u32 (KeyCode) |
|3019|KeyMap   |Custom shortcut      |KeyCOSFollow                    |u32 (KeyCode) |
|3020|KeyMap   |Custom shortcut      |KeyCOSAttack                    |u32 (KeyCode) |
|3021|KeyMap   |Custom shortcut      |KeyCOSAIType                    |u32 (KeyCode) |
|3023|KeyMap   |Custom shortcut      |KeyReplyWhisper                 |u32 (KeyCode) |
|3024|KeyMap   |Custom shortcut      |KeyAutoPotion                   |u32 (KeyCode) |
|3025|KeyMap   |Custom shortcut      |KeyCOSSelection                 |u32 (KeyCode) |
|3026|KeyMap   |Custom shortcut      |KeyPartyMatch                   |u32 (KeyCode) |
|3027|KeyMap   |Custom shortcut      |KeyAlchemy                      |u32 (KeyCode) |
|3029|KeyMap   |Custom shortcut      |KeyTargetEnemy                  |u32 (KeyCode) |
|3030|KeyMap   |Custom shortcut      |KeyTargetRecent                 |u32 (KeyCode) |
|3031|KeyMap   |Custom shortcut      |KeyTargetSupport                |u32 (KeyCode) |
|3032|KeyMap   |Custom shortcut      |KeyTargetSee                    |u32 (KeyCode) |
|3033|KeyMap   |Custom shortcut      |KeyAcademy                      |u32 (KeyCode) |
|3034|KeyMap   |Custom shortcut      |KeyHideFriends                  |u32 (KeyCode) |
|3035|KeyMap   |Custom shortcut      |KeyHideEnemies                  |u32 (KeyCode) |
|3101|KeyMap   |                     |isMouseShortcutSwapped          |bool          |

## Size-exactness proof (openroad, 2026-08-12)

The header above (3 opaque fields, 9 bytes) and every id→width class are now
**arithmetically proven** against a real 681-byte `SROptionSet.dat` reported in
`silkroad-docs/docs/client_startup.md:223-252` — a **reference repository outside this
workspace**, checked out at `<refs>/silkroad-docs/` on the maintainer's machine and
not vendored here (see the source note below):

```
9 (header) + 15*6 + 15*6 + 2*5 + 2*8 + 2*5 + 2*8 + 3*8 + 3*5 + 28*5 + 32*8 + 1*5 = 681
```

exact, where the terms are ids 1-15, 101-115, 501/502, 503/504, 601/602,
603/604, 1001-1003, 1004-1006, 2001-2028, the keymap block, and 3101. That single
identity confirms the 9-byte header, every width class, and that ids
14/15/114/115/2005-2007 (blank names in the CSV) are physically present.

**It also resolves an open question: ids 3010/3022/3028 do not exist** — the
keymap block contributes exactly **32** records (3001-3009, 3011-3021, 3023-3027,
3029-3035), which is what makes the arithmetic land on 681.

**Source note (added 2026-08-15, after the citation was reported as unresolvable).** The
path above is relative to the *reference* checkout, not to this repository, so a reader
searching the openroad tree — or the whole `<projects>` workspace — finds nothing and can
reasonably conclude the derivation rests on a missing file. It does not. The file is at
`<refs>/silkroad-docs/docs/client_startup.md`, line 223 reads
`#### SROptionSet.dat (681 bytes)` verbatim, and lines 223-252 are exactly its
`SROptionSet.dat` section. Re-verified there on 2026-08-15, and the arithmetic re-added
independently:

```
9 + 15*6 + 15*6 + 2*5 + 2*8 + 2*5 + 2*8 + 3*8 + 3*5 + 28*5 + 32*8 + 1*5
= 9 + 90 + 90 + 10 + 16 + 10 + 16 + 24 + 15 + 140 + 256 + 5
= 681
```

and the keymap id list (3001-3009, 3011-3021, 3023-3027, 3029-3035) re-counts to exactly
**32** records, which is the term the identity depends on. So the claim stands as `[V]`
and is **not** downgraded. What remains genuinely outside this repository is the 681-byte
file itself: the byte count is quoted from the reference doc, and nobody has re-measured a
`SROptionSet.dat` on disk here. If that file is ever supplied, `stat -f%z` is the whole
confirmation.

Caveat on the source: `client_startup.md` frames the records as
`[2-byte ID][4-byte value]`, which is wrong — read as the TLV documented above,
its observed id bytes line up exactly. Only its byte counts and hexdumps are
trustworthy. Our parser (`client/src/plugins/settings/sroptionset.rs:48-115`) is
in sync with this doc; it is simply dormant (`import_sroptionset` has no caller).

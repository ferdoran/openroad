use std::f32::consts::PI;
use std::time::Duration;

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::math::{EulerRot, Quat};
use bevy::prelude::{Asset, TypePath, Vec3};
use bevy_tweening::Sequence;
use serde_derive::{Deserialize, Serialize};
use thiserror::Error;

use crate::util::tweening_ext::keyframe_tween;

#[derive(TypePath, Asset, Serialize, Deserialize, Default, Debug, Clone)]
pub struct IntroScene {
    name: String,
    music: String,
    transforms: Vec<CameraKeyframe>,
}

/// One camera key of an `.intro` path.
///
/// An `.intro` file is a transcription of one of the original's
/// `Media/script/intro/<name>.txt` camera blocks (UTF-16LE plain text, **not**
/// JMXVCAMR). Those are SRO data, so **no `.intro` is committed here** —
/// generate one from your own `Media/` with `tools/src/bin/intro_convert.rs`
/// (`assets/intros/*.intro` is gitignored).
///
/// Each key in the source file has **11** whitespace-separated fields:
///
/// ```text
/// 0.0  S_CameraInsert  <frame>  188  95  x y z  rx ry rz  1
/// ```
///
/// We carry **8 of them value-exact** — `frame`, `rx`=188, `rz`=95, the offset
/// xyz and the rotation xyz — and deliberately drop **three**: the leading
/// `0.0`, the `S_CameraInsert` command token, and the trailing `1`. The token is
/// self-evident; the two constants' semantics are **UNKNOWN**
/// (`docs/re/ui/scene-intro-splash.md` §9). The subset is a decision, not a
/// parse loss: nothing in the dropped fields is geometry.
///
/// `IntroScene::music` has no counterpart in that file either — it comes from
/// `Media/config/option.txt:12` (`IntroBGM="maintheme_cut.ogg"`).
#[derive(Serialize, Deserialize, Copy, Clone, Default, Debug)]
pub struct CameraKeyframe {
    pub frame: f32,
    pub rx: f32,
    pub rz: f32,
    pub offset: Vec3,
    pub rotation: Vec3,
}

#[derive(Error, Debug)]
pub enum IntroSceneLoaderError {
    #[error("failed to parse yaml: {0}")]
    YAML(serde_yaml::Error),
    #[error("IO Error: {0}")]
    IO(std::io::Error),
}

impl AssetLoader for IntroScene {
    type Asset = IntroScene;
    type Settings = ();
    type Error = IntroSceneLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        let _ = reader
            .read_to_end(&mut buf)
            .await
            .map_err(|e| IntroSceneLoaderError::IO(e));
        let scene: Self::Asset =
            serde_yaml::from_slice(&buf).map_err(|e| IntroSceneLoaderError::YAML(e))?;
        Ok(scene)
    }

    fn extensions(&self) -> &[&str] {
        &["intro"]
    }
}

impl CameraKeyframe {
    /// Render-space translation of the keyframe: the mirrored SRO-space
    /// position shifted by the floating world origin (see `world_origin`).
    /// Pass `Vec3::ZERO` to get the raw SRO-space position.
    pub fn translation(&self, origin: Vec3) -> Vec3 {
        let base = Vec3::new(self.rx * 1920.0, 0.0, self.rz * 1920.0);
        let offset = base + self.offset;
        let offset = offset * Vec3::new(-1.0, 1.0, 1.0);
        offset - origin
    }

    fn rotation(&self) -> Quat {
        Quat::from_euler(
            EulerRot::XYZ,
            self.rotation.x,
            -self.rotation.y - PI,
            self.rotation.z,
        )
    }
}

/// A camera-script line that does not carry a full `S_CameraInsert` key.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum CameraScriptError {
    #[error("line {line}: S_CameraInsert needs 9 numeric fields, found {found}")]
    ShortKey { line: usize, found: usize },
    #[error("line {line}: field {field} is not a number: {value}")]
    NotANumber {
        line: usize,
        field: usize,
        value: String,
    },
    #[error("no S_CameraInsert key in the script")]
    Empty,
}

impl IntroScene {
    /// Build a scene from an original `Media/script/intro/<name>.txt` camera
    /// block.
    ///
    /// Idea: the four shipped cutscene scripts (`china_wharf`,
    /// `constantinople`, `egypt`, `roc`) are the *same* format the `.intro`
    /// asset already carries — a `[CAMERA]` block of
    /// `0.0 S_CameraInsert <frame> <rx> <rz> <x> <y> <z> <rotx> <roty> <rotz> 1`
    /// rows (`docs/re/ui/cameradata-editor.md`, `docs/re/ui/scene-intro-splash.md`
    /// §3/§6). Only `china_wharf` has ever been transcribed by hand, so the
    /// other three cutscenes are unreachable in openroad purely for want of a
    /// converter. This is that converter: the transcription step stops being
    /// manual, and no user-supplied data has to enter this repository for it
    /// (#569).
    ///
    /// Everything that is not an `S_CameraInsert` row — the `[CAMERA]` header,
    /// other verbs, blank lines — is skipped, because the block is one section
    /// of a larger script and the rest of it is not camera geometry. A row that
    /// *is* a key but cannot be read is an error rather than a skip: silently
    /// dropping a keyframe would shorten a camera path without saying so.
    ///
    /// `music` has no counterpart in these files (the splash's comes from
    /// `Media/config/option.txt:12`), so the caller supplies it.
    pub fn from_camera_script(
        name: &str,
        music: &str,
        script: &str,
    ) -> Result<Self, CameraScriptError> {
        let mut transforms = Vec::new();
        for (index, line) in script.lines().enumerate() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.get(1) != Some(&"S_CameraInsert") {
                continue;
            }
            let line = index + 1;
            // fields[2..11]: frame, rx, rz, x, y, z, rotx, roty, rotz. The
            // leading `0.0`, the verb and the trailing `1` are the three the
            // asset deliberately drops (see `CameraKeyframe`).
            if fields.len() < 11 {
                return Err(CameraScriptError::ShortKey {
                    line,
                    found: fields.len().saturating_sub(2),
                });
            }
            let mut values = [0.0f32; 9];
            for (slot, field) in values.iter_mut().zip(2..11) {
                // The original writes some floats with a C-style `f` suffix
                // (`484.136963f` — in the shipped scripts it is the z offset),
                // which `parse::<f32>` rejects. Strip one trailing `f`/`F`
                // before parsing; without this the converter fails on every
                // real `script/intro/*.txt`.
                let raw = fields[field];
                let text = raw.strip_suffix(['f', 'F']).unwrap_or(raw);
                *slot = text
                    .parse::<f32>()
                    .map_err(|_| CameraScriptError::NotANumber {
                        line,
                        field,
                        value: raw.to_string(),
                    })?;
            }
            transforms.push(CameraKeyframe {
                frame: values[0],
                rx: values[1],
                rz: values[2],
                offset: Vec3::new(values[3], values[4], values[5]),
                rotation: Vec3::new(values[6], values[7], values[8]),
            });
        }
        if transforms.is_empty() {
            return Err(CameraScriptError::Empty);
        }
        Ok(Self {
            name: name.to_string(),
            music: music.to_string(),
            transforms,
        })
    }

    pub fn music(&self) -> &String {
        &self.music
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn get_camera_anim(&self, origin: Vec3) -> Sequence {
        let mut tweens = Vec::with_capacity(self.transforms.len() - 1);

        for i in 0..self.transforms.len() - 1 {
            let start = self.transforms[i];
            let end = self.transforms[i + 1];

            let duration = Duration::from_secs_f32(end.frame - start.frame);

            tweens.push(keyframe_tween(
                start.translation(origin),
                end.translation(origin),
                start.rotation(),
                end.rotation(),
                duration,
            ));
        }

        Sequence::new(tweens)
    }

    /// SRO-space position of the first camera keyframe — the anchor the
    /// floating world origin is derived from when the cinematic starts.
    pub fn start_anchor(&self) -> Option<Vec3> {
        self.transforms.first().map(|kf| kf.translation(Vec3::ZERO))
    }
}

/// Which cutscene the original client plays, and over what music.
///
/// Idea: neither value is ours to invent. The stock client keeps both in
/// `Media/config/option.txt` as `key = "value"` lines, shipping *every*
/// alternative and disabling all but one with a leading `//`. Reading that file
/// rather than hardcoding a name means a user who edits their own option.txt
/// gets the cutscene they asked for, and it is the reason openroad no longer
/// needs a pre-converted `.intro` committed anywhere (ADR 0009: the user's own
/// data is the default reference and the tie-breaker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntroOption {
    /// Archive-relative camera script, e.g. `script/intro/china_wharf.txt`.
    pub script: String,
    /// Music asset path for the cutscene, e.g. `music://maintheme_cut.ogg`.
    pub music: String,
}

/// The track used when `option.txt` names no `IntroBGM`. This is the value the
/// file's own first (commented) entry carries, so a trimmed option.txt still
/// gets the stock splash audio instead of silence.
pub const DEFAULT_INTRO_BGM: &str = "music://maintheme_cut.ogg";

impl IntroOption {
    /// Parse `IntroName` / `IntroBGM` out of a `config/option.txt`.
    ///
    /// The file is plain ASCII with CRLF endings and no BOM. A line is disabled
    /// by a leading `//`, and the shipped file carries one enabled entry per key
    /// among several disabled ones; when more than one is enabled the last wins,
    /// which is the tolerant reading of a hand-edited file.
    ///
    /// `IntroName` is a Windows-separated archive path (`script\intro\x.txt`),
    /// so the separators are normalized here — archive lookup lowercases but
    /// does not translate `\`. `IntroBGM` is a bare filename and becomes a
    /// `music://` asset path.
    ///
    /// Returns `None` when no enabled `IntroName` is present: without a script
    /// there is no cutscene to build, and the caller decides what to do about it.
    pub fn from_option_txt(text: &str) -> Option<Self> {
        let mut script = None;
        let mut music = None;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').trim();
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "IntroName" => script = Some(value.replace('\\', "/")),
                "IntroBGM" => music = Some(format!("music://{value}")),
                _ => {}
            }
        }
        Some(Self {
            script: script?,
            music: music.unwrap_or_else(|| DEFAULT_INTRO_BGM.to_string()),
        })
    }

    /// The scene name for a script path — its file stem (`script/intro/roc.txt`
    /// -> `roc`), which is also the stem of the optional `.intro` override.
    pub fn name(&self) -> &str {
        self.script
            .rsplit('/')
            .next()
            .unwrap_or(&self.script)
            .strip_suffix(".txt")
            .unwrap_or("intro")
    }
}

#[cfg(test)]
mod tests {
    use crate::assets::intro_scene::{
        CameraScriptError, IntroOption, IntroScene, DEFAULT_INTRO_BGM,
    };
    /// A `config/option.txt` in the original's shape: `key = "value"` lines,
    /// every alternative shipped with all but one disabled by a leading `//`.
    /// The names are **synthetic**; what is under test is the selection rule and
    /// the separator/prefix normalization, not any particular cutscene.
    const SYNTHETIC_OPTION_TXT: &str = "\
StartProcess = \"something\"\r
//IntroName = \"script\\intro\\disabled_one.txt\"\r
//IntroBGM = \"disabled_one.ogg\"\r
IntroName = \"script\\intro\\chosen.txt\"\r
IntroBGM = \"chosen.ogg\"\r
";

    /// The enabled entry wins over the disabled ones, backslashes become
    /// archive separators, and the bare BGM filename becomes a `music://` path.
    #[test]
    fn option_txt_selects_the_enabled_entry_and_normalizes_it() {
        let option = IntroOption::from_option_txt(SYNTHETIC_OPTION_TXT)
            .expect("an enabled IntroName yields an option");

        assert_eq!(option.script, "script/intro/chosen.txt");
        assert_eq!(option.music, "music://chosen.ogg");
        assert_eq!(option.name(), "chosen");
    }

    /// A file with no `IntroBGM` still plays the stock splash track rather than
    /// falling silent.
    #[test]
    fn option_txt_without_bgm_falls_back_to_the_stock_track() {
        let option = IntroOption::from_option_txt("IntroName = \"script\\intro\\x.txt\"\n")
            .expect("an enabled IntroName yields an option");

        assert_eq!(option.music, DEFAULT_INTRO_BGM);
    }

    /// No enabled `IntroName` means there is no cutscene to build, and the
    /// caller — not this parser — decides what to do about that.
    #[test]
    fn option_txt_without_an_enabled_intro_name_is_none() {
        assert!(IntroOption::from_option_txt("//IntroName = \"script\\intro\\x.txt\"\n").is_none());
        assert!(IntroOption::from_option_txt("StartProcess = \"y\"\n").is_none());
    }

    /// A hand-edited file with several entries left enabled takes the last, so
    /// uncommenting one below the others does what it looks like it does.
    #[test]
    fn option_txt_takes_the_last_enabled_entry() {
        let option = IntroOption::from_option_txt(
            "IntroName = \"script\\intro\\first.txt\"\nIntroName = \"script\\intro\\second.txt\"\n",
        )
        .expect("an enabled IntroName yields an option");

        assert_eq!(option.name(), "second");
    }

    /// A camera block in the original's per-key field layout:
    ///
    /// ```text
    /// 0.0  S_CameraInsert  <frame>  188  95  x y z  rx ry rz  1
    /// ```
    ///
    /// The values are **synthetic** — deliberately not the original's. What is
    /// under test here is the converter's field mapping, not any particular
    /// cutscene: which of the 12 tokens are carried (`frame`, `rx`, `rz`, the
    /// offset xyz and the rotation xyz) and which three are dropped (the
    /// leading `0.0`, the command token and the trailing `1`). Real camera data
    /// belongs to the user's own `Media/` and is converted by
    /// `tools/src/bin/intro_convert.rs`, never committed here.
    const SYNTHETIC_CAMERA_KEYS: &str = "\
0.0 S_CameraInsert 0.0 188 95 100.500000 10.250000 -20.750000 -0.100000 -1.500000 0.000000 1
0.0 S_CameraInsert 5.0 188 95 200.250000 11.500000 -40.500000 -0.200000 -2.500000 0.000000 1
0.0 S_CameraInsert 10.0 188 95 300.125000 12.750000 -60.250000 -0.300000 -3.500000 0.000000 1
";

    /// The converter carries exactly the 8 fields the loader documents and
    /// drops exactly the other three.
    #[test]
    fn converter_carries_the_eight_fields_and_drops_the_other_three() {
        let converted =
            IntroScene::from_camera_script("synthetic", "music://test.ogg", SYNTHETIC_CAMERA_KEYS)
                .expect("a well-formed camera block converts");

        assert_eq!(converted.name, "synthetic");
        assert_eq!(converted.music, "music://test.ogg");
        assert_eq!(converted.transforms.len(), 3);

        let expected: Vec<Vec<f32>> = SYNTHETIC_CAMERA_KEYS
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                assert_eq!(f[1], "S_CameraInsert");
                assert_eq!(*f.last().expect("trailing field"), "1");
                f[2..10]
                    .iter()
                    .map(|x| x.parse::<f32>().expect("numeric"))
                    .collect()
            })
            .collect();

        for (key, (got, want)) in converted.transforms.iter().zip(&expected).enumerate() {
            assert_eq!(got.frame, want[0], "key {key} frame");
            assert_eq!(got.rx, want[1], "key {key} rx");
            assert_eq!(got.rz, want[2], "key {key} rz");
            assert_eq!(got.offset[0], want[3], "key {key} offset.x");
            assert_eq!(got.offset[1], want[4], "key {key} offset.y");
            assert_eq!(got.offset[2], want[5], "key {key} offset.z");
            assert_eq!(got.rotation[0], want[6], "key {key} rotation.x");
            assert_eq!(got.rotation[1], want[7], "key {key} rotation.y");
        }
    }

    /// A camera block is one section of a larger script: the header, the other
    /// verbs and blank lines are skipped, and only the keys are read.
    #[test]
    fn converter_reads_only_the_camera_insert_rows() {
        let script = format!(
            "[CAMERA]\n\n0.0 S_SoundPlay 0.0 maintheme_cut.ogg\n{SYNTHETIC_CAMERA_KEYS}[END]\n"
        );
        let scene = IntroScene::from_camera_script("x", "music://y.ogg", &script)
            .expect("the camera block still converts");
        assert_eq!(scene.transforms.len(), 3);
    }

    /// The original writes some floats with a C-style `f` suffix — the z offset
    /// in every shipped `script/intro/*.txt`. Before this was handled the
    /// converter failed on real input with "field 7 is not a number".
    #[test]
    fn converter_accepts_the_originals_f_suffixed_floats() {
        let script = "0.0 S_CameraInsert 0.0 188 95 1.5 2.5 3.5f -0.1 -0.2 0.0 1\n";
        let scene = IntroScene::from_camera_script("x", "music://y.ogg", script)
            .expect("an f-suffixed float parses");
        assert_eq!(scene.transforms.len(), 1);
        assert_eq!(scene.transforms[0].offset[2], 3.5);

        // a suffix that is not a trailing `f` is still an error
        let bad = "0.0 S_CameraInsert 0.0 188 95 1.5 2.5 3.5ff -0.1 -0.2 0.0 1\n";
        assert!(IntroScene::from_camera_script("x", "music://y.ogg", bad).is_err());
    }

    /// A row that IS a key but cannot be read is an error, not a silent skip —
    /// dropping it would shorten the camera path without saying so.
    #[test]
    fn converter_refuses_to_drop_an_unreadable_key() {
        let short = "0.0 S_CameraInsert 0.0 188 95 1.0 2.0 3.0\n";
        assert!(matches!(
            IntroScene::from_camera_script("x", "y", short),
            Err(CameraScriptError::ShortKey { line: 1, found: 6 })
        ));
        let bad = "0.0 S_CameraInsert 0.0 188 95 1.0 2.0 3.0 4.0 nan? 0.0 1\n";
        assert!(matches!(
            IntroScene::from_camera_script("x", "y", bad),
            Err(CameraScriptError::NotANumber { line: 1, .. })
        ));
        assert!(matches!(
            IntroScene::from_camera_script("x", "y", "[CAMERA]\n"),
            Err(CameraScriptError::Empty)
        ));
    }

    #[test]
    fn parse_intro_scene() {
        let yaml = r#"
name: synthetic
music: "music://test.ogg"
transforms:
  - frame: 0.0
    rx: 188.0
    rz: 95.0
    offset:
      - 100.500000
      - 10.250000
      - -20.750000
    rotation:
      - -0.100000
      - -1.500000
      - 0.0
  - frame: 5.0
    rx: 188.0
    rz: 95.0
    offset:
      - 200.250000
      - 11.500000
      - -40.500000
    rotation:
      - -0.200000
      - -2.500000
      - 0.0
  - frame: 10.0
    rx: 188.0
    rz: 95.0
    offset:
      - 300.125000
      - 12.750000
      - -60.250000
    rotation:
      - -0.300000
      - -3.500000
      - 0.0
        "#;

        let parsed = serde_yaml::from_str::<IntroScene>(&yaml);
        println!("{:?}", parsed);
        assert_eq!(false, parsed.is_err());
        let parsed = parsed.unwrap();

        assert_eq!("music://test.ogg", parsed.music);
        assert_eq!(3, parsed.transforms.len());
    }
}

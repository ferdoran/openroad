//! Byte-level parser for JMXVEFF ".efp" effect files.
//!
//! Layout per the SilkroadDoc wiki (JMXVEFF page), corrected where real game
//! data disagrees:
//! - `EFStaticEmit` is 20 bytes (4 x u32 + f32), not 16.
//! - The `DiffuseGraph` controller carries no trailing float pair.
//! - The timeline tail between the program source list and the view-mode
//!   source is 6 bytes; the following view-mode source name is validated to
//!   confirm the position rather than to search for it.
//!
//! The `start`/`end`/`float2` fields of `EESourceData` frequently contain
//! uninitialized memory (garbage pointers written to disk by the original
//! exporter), so they are stored as raw u32 with an f32 view.
//!
//! This module is self-contained on purpose (std + bevy::math only) so that
//! `tools/src/bin/efp_scan` can include it via `#[path]` without linking the
//! client binary.

use std::fmt;

use bevy::math::{Mat4, Vec3, Vec4};

pub const VIEW_NAMES: [&str; 4] = [
    "ViewNone",
    "ViewBillboard",
    "ViewVBillboard",
    "ViewYBillboard",
];
pub const RENDER_NAMES: [&str; 6] = [
    "RenderNone",
    "RenderPlate",
    "RenderMesh",
    "RenderLinkPipe",
    "RenderLinkDPipe",
    "RenderLinkObj",
];

const MAX_STRING_LEN: u32 = 4096;
const MAX_LIST_LEN: u32 = 4096;
const MAX_CHILDREN: u32 = 512;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub offset: usize,
    pub what: String,
    /// Last successfully read name/command string, for orientation.
    pub marker: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "offset {:#x}: {} (after \"{}\")",
            self.offset, self.what, self.marker
        )
    }
}

impl std::error::Error for ParseError {}

pub struct Rd<'a> {
    data: &'a [u8],
    pos: usize,
    last_marker: String,
}

impl<'a> Rd<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            last_marker: String::new(),
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn err(&self, what: impl Into<String>) -> ParseError {
        ParseError {
            offset: self.pos,
            what: what.into(),
            marker: self.last_marker.clone(),
        }
    }

    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], ParseError> {
        if self.remaining() < n {
            return Err(self.err(format!("unexpected EOF reading {what} ({n} bytes)")));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    pub fn u8(&mut self, what: &str) -> Result<u8, ParseError> {
        Ok(self.take(1, what)?[0])
    }

    pub fn u32(&mut self, what: &str) -> Result<u32, ParseError> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self, what: &str) -> Result<i32, ParseError> {
        Ok(self.u32(what)? as i32)
    }

    pub fn f32(&mut self, what: &str) -> Result<f32, ParseError> {
        Ok(f32::from_bits(self.u32(what)?))
    }

    pub fn vec3(&mut self, what: &str) -> Result<Vec3, ParseError> {
        Ok(Vec3::new(self.f32(what)?, self.f32(what)?, self.f32(what)?))
    }

    pub fn vec4(&mut self, what: &str) -> Result<Vec4, ParseError> {
        Ok(Vec4::new(
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
        ))
    }

    /// JMX 64-byte matrices are row-major `D3DXMATRIX` bytes, and reading the
    /// four rows as glam COLUMNS is deliberate, not a mix-up: D3DX matrices
    /// are built for row-vector math (`v·M`) while glam applies `M·v`, and
    /// `M_d3dᵀ·v ≡ v·M_d3d` — the storage-order flip and the vector-convention
    /// flip cancel exactly (pinned by `mat4_reads_d3d_rows_as_glam_columns`).
    pub fn mat4(&mut self, what: &str) -> Result<Mat4, ParseError> {
        Ok(Mat4::from_cols(
            self.vec4(what)?,
            self.vec4(what)?,
            self.vec4(what)?,
            self.vec4(what)?,
        ))
    }

    /// u32-length-prefixed latin1 string (the JMX convention).
    pub fn string(&mut self, what: &str) -> Result<String, ParseError> {
        let len = self.u32(what)?;
        if len > MAX_STRING_LEN {
            return Err(self.err(format!("implausible string length {len} for {what}")));
        }
        let bytes = self.take(len as usize, what)?;
        let s: String = bytes
            .iter()
            .filter(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();
        self.last_marker = s.clone();
        Ok(s)
    }
}

#[derive(Debug, Clone, Default)]
pub struct EeBlend<T> {
    pub begin: f32,
    pub end: f32,
    pub keys: Vec<(f32, T)>,
}

impl<T: Copy> EeBlend<T> {
    /// Linearly interpolate the keyframes at `t` with `lerp(a, b, s)`.
    pub fn sample(&self, t: f32, lerp: impl Fn(T, T, f32) -> T) -> Option<T> {
        let (&(t0, v0), &(tn, vn)) = (self.keys.first()?, self.keys.last()?);
        if t <= t0 {
            return Some(v0);
        }
        if t >= tn {
            return Some(vn);
        }
        let idx = self.keys.partition_point(|&(kt, _)| kt <= t);
        let (ta, va) = self.keys[idx - 1];
        let (tb, vb) = self.keys[idx];
        let span = tb - ta;
        if span <= f32::EPSILON {
            return Some(va);
        }
        Some(lerp(va, vb, (t - ta) / span))
    }
}

fn blend<'a, T>(
    rd: &mut Rd<'a>,
    what: &str,
    mut value: impl FnMut(&mut Rd<'a>, &str) -> Result<T, ParseError>,
) -> Result<EeBlend<T>, ParseError> {
    let begin = rd.f32(what)?;
    let end = rd.f32(what)?;
    let count = rd.u32(what)?;
    if count > MAX_LIST_LEN {
        return Err(rd.err(format!("implausible blend key count {count} for {what}")));
    }
    let mut keys = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let t = rd.f32(what)?;
        keys.push((t, value(rd, what)?));
    }
    Ok(EeBlend { begin, end, keys })
}

/// ARGB32 color as stored in the file.
pub type Argb = u32;

pub fn argb_channels(argb: Argb) -> [u8; 4] {
    [
        (argb >> 24) as u8, // A
        (argb >> 16) as u8, // R
        (argb >> 8) as u8,  // G
        argb as u8,         // B
    ]
}

/// Static emitter parameters. 20 bytes in real files (wiki claims 16).
/// Exe-RE confirmed: ints = [start_frame, window_frames, period_frames,
/// max_alive], spawn_rate = particles per emission tick (accumulator with
/// death refund). See docs/formats/efp-jmxveff.md § EFStaticEmit.
#[derive(Debug, Clone, Copy, Default)]
pub struct EfStaticEmit {
    pub ints: [u32; 4],
    pub spawn_rate: f32,
}

impl EfStaticEmit {
    fn parse(rd: &mut Rd) -> Result<Self, ParseError> {
        Ok(Self {
            ints: [
                rd.u32("StaticEmit")?,
                rd.u32("StaticEmit")?,
                rd.u32("StaticEmit")?,
                rd.u32("StaticEmit")?,
            ],
            spawn_rate: rd.f32("StaticEmit")?,
        })
    }
}

/// Axis-angle rotation: unit vector + angle in degrees, plus its matrix form.
#[derive(Debug, Clone, Copy)]
pub struct AxisVector4 {
    pub axis_angle: Vec4,
    pub matrix: Mat4,
}

/// Euler rotation (pitch/yaw/roll degrees) plus its matrix form.
#[derive(Debug, Clone, Copy)]
pub struct RotVector {
    pub euler_degrees: Vec3,
    pub matrix: Mat4,
}

/// Cone (exe RE): XY = random speed min/max pair (copied verbatim to both
/// vecs), Z = cone half-angle (degrees left, radians right).
#[derive(Debug, Clone, Copy)]
pub struct AngleVector1 {
    pub degrees: Vec3,
    pub radians: Vec3,
}

#[derive(Debug, Clone)]
pub struct FrameTextureSlide {
    pub left: Vec3,
    pub frames: Vec<Vec4>,
}

#[derive(Debug, Clone)]
pub enum EffectCommand {
    // Lifetime
    NeverExtinct,
    NormalTimeExtinct,
    NormalTimeLoop,
    // Emission
    StaticEmit(EfStaticEmit),
    ProgramUpdate,
    // Rotation
    SetRotationMat(Mat4),
    SetRVelocityMat(Mat4),
    SetRotation(RotVector),
    SetRVelocity(RotVector),
    SetRotationAxis(AxisVector4),
    SetRVelocityAxis(AxisVector4),
    SetBanRot(Vec<Mat4>),
    // Position / velocity / forces
    SetPosition(Vec3),
    SetSpherePos(Vec3),
    SetConePos(AngleVector1),
    SetBanPos(Vec<Vec3>),
    SetVelocity(Vec3),
    SetConeVel(AngleVector1),
    Force(Vec3),
    ConeForce(AngleVector1),
    Attraction(f32),
    // Decoration
    TextureSlide(FrameTextureSlide),
    SetGraphScale(Vec<Vec3>),
    SetGraphRandomScale(u32),
    SetGraphDiffuse(Vec<Argb>),
    SetShapeRot(AxisVector4),
    SetShapeRotVel(AxisVector4),
    // View / render markers used as source commands
    ViewNone,
    ViewBillboard,
    ViewVBillboard,
    ViewYBillboard,
    RenderNone,
    RenderPlate,
    RenderMesh,
    RenderLinkPipe,
    RenderLinkDPipe,
    RenderLinkObj,
}

impl EffectCommand {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NeverExtinct => "NeverExtinct",
            Self::NormalTimeExtinct => "NormalTimeExtinct",
            Self::NormalTimeLoop => "NormalTimeLoop",
            Self::StaticEmit(_) => "StaticEmit",
            Self::ProgramUpdate => "ProgramUpdate",
            Self::SetRotationMat(_) => "SetRotationMat",
            Self::SetRVelocityMat(_) => "SetRVelocityMat",
            Self::SetRotation(_) => "SetRotation",
            Self::SetRVelocity(_) => "SetRVelocity",
            Self::SetRotationAxis(_) => "SetRotationAxis",
            Self::SetRVelocityAxis(_) => "SetRVelocityAxis",
            Self::SetBanRot(_) => "SetBANRot",
            Self::SetPosition(_) => "SetPosition",
            Self::SetSpherePos(_) => "SetSpherePos",
            Self::SetConePos(_) => "SetConePos",
            Self::SetBanPos(_) => "SetBANPos",
            Self::SetVelocity(_) => "SetVelocity",
            Self::SetConeVel(_) => "SetConeVel",
            Self::Force(_) => "Force",
            Self::ConeForce(_) => "ConeForce",
            Self::Attraction(_) => "Attraction",
            Self::TextureSlide(_) => "TextureSlide",
            Self::SetGraphScale(_) => "SetGraphScale",
            Self::SetGraphRandomScale(_) => "SetGraphRandomScale",
            Self::SetGraphDiffuse(_) => "SetGraphDiffuse",
            Self::SetShapeRot(_) => "SetShapeRot",
            Self::SetShapeRotVel(_) => "SetShapeRotVel",
            Self::ViewNone => "ViewNone",
            Self::ViewBillboard => "ViewBillboard",
            Self::ViewVBillboard => "ViewVBillboard",
            Self::ViewYBillboard => "ViewYBillboard",
            Self::RenderNone => "RenderNone",
            Self::RenderPlate => "RenderPlate",
            Self::RenderMesh => "RenderMesh",
            Self::RenderLinkPipe => "RenderLinkPipe",
            Self::RenderLinkDPipe => "RenderLinkDPipe",
            Self::RenderLinkObj => "RenderLinkObj",
        }
    }
}

fn axis_vector4(rd: &mut Rd, what: &str) -> Result<AxisVector4, ParseError> {
    Ok(AxisVector4 {
        axis_angle: rd.vec4(what)?,
        matrix: rd.mat4(what)?,
    })
}

fn rot_vector(rd: &mut Rd, what: &str) -> Result<RotVector, ParseError> {
    Ok(RotVector {
        euler_degrees: rd.vec3(what)?,
        matrix: rd.mat4(what)?,
    })
}

fn angle_vector1(rd: &mut Rd, what: &str) -> Result<AngleVector1, ParseError> {
    Ok(AngleVector1 {
        degrees: rd.vec3(what)?,
        radians: rd.vec3(what)?,
    })
}

fn counted<'a, T>(
    rd: &mut Rd<'a>,
    what: &str,
    mut value: impl FnMut(&mut Rd<'a>, &str) -> Result<T, ParseError>,
) -> Result<Vec<T>, ParseError> {
    let count = rd.u32(what)?;
    if count > MAX_LIST_LEN {
        return Err(rd.err(format!("implausible count {count} for {what}")));
    }
    (0..count).map(|_| value(rd, what)).collect()
}

fn parse_command(rd: &mut Rd, name: &str) -> Result<EffectCommand, ParseError> {
    use EffectCommand as C;
    Ok(match name {
        "NeverExtinct" => C::NeverExtinct,
        "NormalTimeExtinct" => C::NormalTimeExtinct,
        "NormalTimeLoop" => C::NormalTimeLoop,
        "StaticEmit" => C::StaticEmit(EfStaticEmit::parse(rd)?),
        "ProgramUpdate" => C::ProgramUpdate,
        "SetRotationMat" => C::SetRotationMat(rd.mat4(name)?),
        "SetRVelocityMat" => C::SetRVelocityMat(rd.mat4(name)?),
        "SetRotation" => C::SetRotation(rot_vector(rd, name)?),
        "SetRVelocity" => C::SetRVelocity(rot_vector(rd, name)?),
        "SetRotationAxis" => C::SetRotationAxis(axis_vector4(rd, name)?),
        "SetRVelocityAxis" => C::SetRVelocityAxis(axis_vector4(rd, name)?),
        // Real files carry an extra u32 before the keyframe count that the
        // wiki's FrameBANRotation/FrameBANPosition layouts don't mention.
        "SetBANRot" => {
            let _unk = rd.u32(name)?;
            C::SetBanRot(counted(rd, name, |rd, w| rd.mat4(w))?)
        }
        "SetPosition" => C::SetPosition(rd.vec3(name)?),
        "SetSpherePos" => C::SetSpherePos(rd.vec3(name)?),
        "SetConePos" => C::SetConePos(angle_vector1(rd, name)?),
        "SetBANPos" => {
            let _unk = rd.u32(name)?;
            C::SetBanPos(counted(rd, name, |rd, w| rd.vec3(w))?)
        }
        "SetVelocity" => C::SetVelocity(rd.vec3(name)?),
        "SetConeVel" => C::SetConeVel(angle_vector1(rd, name)?),
        "Force" => C::Force(rd.vec3(name)?),
        "ConeForce" => C::ConeForce(angle_vector1(rd, name)?),
        "Attraction" => C::Attraction(rd.f32(name)?),
        "TextureSlide" => C::TextureSlide(FrameTextureSlide {
            left: rd.vec3(name)?,
            frames: counted(rd, name, |rd, w| rd.vec4(w))?,
        }),
        "SetGraphScale" => C::SetGraphScale(counted(rd, name, |rd, w| rd.vec3(w))?),
        "SetGraphRandomScale" => C::SetGraphRandomScale(rd.u32(name)?),
        "SetGraphDiffuse" => C::SetGraphDiffuse(counted(rd, name, |rd, w| rd.u32(w))?),
        "SetShapeRot" => C::SetShapeRot(axis_vector4(rd, name)?),
        "SetShapeRotVel" => C::SetShapeRotVel(axis_vector4(rd, name)?),
        "ViewNone" => C::ViewNone,
        "ViewBillboard" => C::ViewBillboard,
        "ViewVBillboard" => C::ViewVBillboard,
        "ViewYBillboard" => C::ViewYBillboard,
        "RenderNone" => C::RenderNone,
        "RenderPlate" => C::RenderPlate,
        "RenderMesh" => C::RenderMesh,
        "RenderLinkPipe" => C::RenderLinkPipe,
        "RenderLinkDPipe" => C::RenderLinkDPipe,
        "RenderLinkObj" => C::RenderLinkObj,
        other => return Err(rd.err(format!("unknown command \"{other}\""))),
    })
}

/// One authored command with its execution schedule. Exe RE (serializer
/// 0xca29d0, decoder 0xca0f40, row compiler 0xca9070): the command occupies
/// the effect frames `trunc(start + k·step)` up to `end` inclusive and
/// executes once per occupied frame; `mode` selects how the three floats
/// decode (absolute frames, percent of program length, relative, count —
/// see `schedule()` in the runtime). `variant` picks the command's
/// link-space flavor (runtime id = base + variant).
#[derive(Debug, Clone)]
pub struct EeSourceData {
    pub command: EffectCommand,
    pub variant: u8,
    pub mode: u8,
    pub start: f32,
    pub step: f32,
    pub end: f32,
}

fn parse_source_data(rd: &mut Rd) -> Result<Option<EeSourceData>, ParseError> {
    let has_data = rd.u8("EESourceData.hasData")?;
    match has_data {
        0 => Ok(None),
        1 => {
            let name = rd.string("EESourceData.commandName")?;
            let variant = rd.u8("EESourceData.variant")?;
            let mode = rd.u8("EESourceData.mode")?;
            let start = f32::from_bits(rd.u32("EESourceData.start")?);
            let step = f32::from_bits(rd.u32("EESourceData.step")?);
            let end = f32::from_bits(rd.u32("EESourceData.end")?);
            let command = parse_command(rd, &name)?;
            Ok(Some(EeSourceData {
                command,
                variant,
                mode,
                start,
                step,
                end,
            }))
        }
        other => Err(rd.err(format!("bad EESourceData.hasData byte {other}"))),
    }
}

fn parse_source_list(rd: &mut Rd, what: &str) -> Result<Vec<EeSourceData>, ParseError> {
    let count = rd.u32(what)?;
    if count > MAX_LIST_LEN {
        return Err(rd.err(format!("implausible source list count {count} for {what}")));
    }
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if let Some(data) = parse_source_data(rd)? {
            out.push(data);
        }
    }
    Ok(out)
}

/// D3D9 render/texture state + mesh/texture references of one node.
#[derive(Debug, Clone, Default)]
pub struct EeResource {
    /// `D3DCULL`, not a bool: 1 NONE (two-sided), 2 CW, 3 CCW. The corpus
    /// holds exactly those three values and never 0, so reading it as
    /// `!= 0` would call every node two-sided.
    pub cull_mode: u32,
    /// D3DBLEND constants.
    pub src_blend: u32,
    pub dst_blend: u32,
    /// SrcTextureArg1/Arg2/OP, DstTextureArg1/Arg2/OP (D3DTA / D3DTEXTUREOP).
    pub texture_stage: [u32; 6],
    /// (mesh .bms path — may be empty, texture .ddj paths).
    pub meshes: Vec<(String, Vec<String>)>,
}

impl EeResource {
    /// All non-empty texture paths, as stored (backslashes, original case).
    pub fn texture_paths(&self) -> impl Iterator<Item = &str> {
        self.meshes
            .iter()
            .flat_map(|(_, t)| t.iter())
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// All non-empty mesh paths.
    pub fn mesh_paths(&self) -> impl Iterator<Item = &str> {
        self.meshes
            .iter()
            .map(|(m, _)| m.as_str())
            .filter(|s| !s.is_empty())
    }
}

const MAX_BLEND_CONST: u32 = 17; // D3DBLEND_INVSRCCOLOR2 is 17 in later SDKs

fn parse_resource(rd: &mut Rd) -> Result<EeResource, ParseError> {
    let cull_mode = rd.u32("EEResource.CullMode")?;
    let src_blend = rd.u32("EEResource.SrcBlend")?;
    let dst_blend = rd.u32("EEResource.DstBlend")?;
    if !(1..=MAX_BLEND_CONST).contains(&src_blend) || !(1..=MAX_BLEND_CONST).contains(&dst_blend) {
        return Err(rd.err(format!(
            "implausible blend constants src={src_blend} dst={dst_blend}"
        )));
    }
    let mut texture_stage = [0u32; 6];
    for slot in &mut texture_stage {
        *slot = rd.u32("EEResource.textureStage")?;
    }
    let mesh_count = rd.u32("EEResource.meshCount")?;
    if mesh_count > 64 {
        return Err(rd.err(format!("implausible mesh count {mesh_count}")));
    }
    let mut meshes = Vec::with_capacity(mesh_count as usize);
    for _ in 0..mesh_count {
        let mesh_path = rd.string("EEResource.meshPath")?;
        let tex_count = rd.u32("EEResource.textureCount")?;
        if tex_count > 64 {
            return Err(rd.err(format!("implausible texture count {tex_count}")));
        }
        let textures = (0..tex_count)
            .map(|_| rd.string("EEResource.texturePath"))
            .collect::<Result<Vec<_>, _>>()?;
        meshes.push((mesh_path, textures));
    }
    Ok(EeResource {
        cull_mode,
        src_blend,
        dst_blend,
        texture_stage,
        meshes,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    None,
    Billboard,
    VBillboard,
    YBillboard,
}

impl ViewMode {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "ViewNone" => Self::None,
            "ViewBillboard" => Self::Billboard,
            "ViewVBillboard" => Self::VBillboard,
            "ViewYBillboard" => Self::YBillboard,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderShape {
    #[default]
    None,
    Plate,
    Mesh,
    LinkPipe,
    LinkDPipe,
    LinkObj,
}

impl RenderShape {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "RenderNone" => Self::None,
            "RenderPlate" => Self::Plate,
            "RenderMesh" => Self::Mesh,
            "RenderLinkPipe" => Self::LinkPipe,
            "RenderLinkDPipe" => Self::LinkDPipe,
            "RenderLinkObj" => Self::LinkObj,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub enum EeParameter {
    BlendScaleGraph(EeBlend<Vec3>),
    BlendDiffuseGraph(EeBlend<Argb>),
    BsAnimation(Vec<String>),
}

fn parse_global_parameter(rd: &mut Rd, name: &str) -> Result<EeParameter, ParseError> {
    Ok(match name {
        "BlendScaleGraph" => EeParameter::BlendScaleGraph(blend(rd, name, |rd, w| rd.vec3(w))?),
        "BlendDiffuseGraph" => EeParameter::BlendDiffuseGraph(blend(rd, name, |rd, w| rd.u32(w))?),
        "BSAnimation" => EeParameter::BsAnimation(counted(rd, name, |rd, w| rd.string(w))?),
        other => return Err(rd.err(format!("unknown global parameter \"{other}\""))),
    })
}

#[derive(Debug, Clone)]
pub enum EfController {
    NormalTimeLife,
    NormalTimeLoopLife,
    StaticEmit(EfStaticEmit),
    Program(Vec<EeSourceData>),
    LinkMode([i32; 4]),
    Ban(Vec<String>),
    ViewMode(ViewMode),
    Shape {
        shape: RenderShape,
        resource: EeResource,
    },
    ScaleGraph {
        x: EeBlend<f32>,
        y: EeBlend<f32>,
        z: EeBlend<f32>,
        float0: f32,
        float1: f32,
    },
    DiffuseGraph {
        alpha: EeBlend<u8>,
        color: EeBlend<Argb>,
    },
}

fn parse_controller(rd: &mut Rd, name: &str) -> Result<EfController, ParseError> {
    Ok(match name {
        "NormalTimeLife" => EfController::NormalTimeLife,
        "NormalTimeLoopLife" => EfController::NormalTimeLoopLife,
        "StaticEmit" => EfController::StaticEmit(EfStaticEmit::parse(rd)?),
        "Program" => EfController::Program(parse_source_list(rd, "Program controller")?),
        "LinkMode" => {
            EfController::LinkMode([rd.i32(name)?, rd.i32(name)?, rd.i32(name)?, rd.i32(name)?])
        }
        "BAN" => EfController::Ban(counted(rd, name, |rd, w| rd.string(w))?),
        "ViewMode" => {
            let mode = rd.string(name)?;
            EfController::ViewMode(
                ViewMode::from_name(&mode)
                    .ok_or_else(|| rd.err(format!("unknown view mode \"{mode}\"")))?,
            )
        }
        "Shape" => {
            let shape_name = rd.string(name)?;
            let shape = RenderShape::from_name(&shape_name)
                .ok_or_else(|| rd.err(format!("unknown render shape \"{shape_name}\"")))?;
            EfController::Shape {
                shape,
                resource: parse_resource(rd)?,
            }
        }
        "ScaleGraph" => EfController::ScaleGraph {
            x: blend(rd, name, |rd, w| rd.f32(w))?,
            y: blend(rd, name, |rd, w| rd.f32(w))?,
            z: blend(rd, name, |rd, w| rd.f32(w))?,
            float0: rd.f32(name)?,
            float1: rd.f32(name)?,
        },
        // Verified against real data: no trailing float pair (wiki disagrees).
        "DiffuseGraph" => EfController::DiffuseGraph {
            alpha: blend(rd, name, |rd, w| rd.u8(w))?,
            color: blend(rd, name, |rd, w| rd.u32(w))?,
        },
        other => return Err(rd.err(format!("unknown controller \"{other}\""))),
    })
}

/// Timeline/attachment metadata of a node. The byte layout between the
/// program source list and the view-mode source is ambiguous in the wiki;
/// `pad` holds the disambiguated slack bytes verbatim.
#[derive(Debug, Clone, Default)]
pub struct NodeTimeline {
    pub byte0: u8,
    pub byte1: u8,
    /// start / end / third int (frame window per wiki).
    pub ints: [u32; 3],
    /// Remaining bytes before the view-mode source (contains the
    /// AttachToParent int per wiki; layout resolved by marker validation).
    pub pad: Vec<u8>,
}

impl NodeTimeline {
    pub fn start_frame(&self) -> u32 {
        self.ints[0]
    }

    pub fn end_frame(&self) -> u32 {
        self.ints[1]
    }

    /// Best-effort AttachToParent flag: the u32 that sits mid-pad per wiki.
    pub fn attach_to_parent(&self) -> bool {
        match self.pad.len() {
            6 => u32::from_le_bytes([self.pad[1], self.pad[2], self.pad[3], self.pad[4]]) != 0,
            7 => u32::from_le_bytes([self.pad[2], self.pad[3], self.pad[4], self.pad[5]]) != 0,
            _ => self.pad.iter().any(|&b| b != 0),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EfStoredObject {
    pub data_offset: u32,
    pub name: String,
    pub controllers: Vec<EfController>,
    /// The node's program length in effect frames (exe RE: the u32 at
    /// node+0x7c, serialized first in the runtime block 0xca45f0) — the
    /// instance lifetime (NormalTimeExtinct kills at this age), the loop
    /// modulus, the row count graphs bake to, and the base percent-mode
    /// command schedules resolve against.
    pub program_len: u32,
    pub global_params: Vec<(String, EeParameter)>,
    pub emitters: Vec<EeSourceData>,
    pub lifetime: Option<EeSourceData>,
    pub programs: Vec<EeSourceData>,
    pub timeline: NodeTimeline,
    pub view_mode: ViewMode,
    pub resource: EeResource,
    pub render_shape: RenderShape,
    pub decorations: Vec<EeSourceData>,
    /// Indices into `EfStoredEffect::nodes`.
    pub children: Vec<usize>,
}

impl EfStoredObject {
    pub fn static_emit(&self) -> Option<&EfStaticEmit> {
        self.controllers.iter().find_map(|c| match c {
            EfController::StaticEmit(e) => Some(e),
            _ => None,
        })
    }

    pub fn scale_graph(&self) -> Option<(&EeBlend<f32>, &EeBlend<f32>, &EeBlend<f32>)> {
        self.controllers.iter().find_map(|c| match c {
            EfController::ScaleGraph { x, y, z, .. } => Some((x, y, z)),
            _ => None,
        })
    }

    pub fn diffuse_graph(&self) -> Option<(&EeBlend<u8>, &EeBlend<Argb>)> {
        self.controllers.iter().find_map(|c| match c {
            EfController::DiffuseGraph { alpha, color } => Some((alpha, color)),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EfpVersion {
    V11,
    V12,
    V13,
}

#[derive(Debug, Clone, Default)]
pub struct EfStoredEffect {
    pub version_str: String,
    pub root_scale: f32,
    pub v13_ints: [i32; 3],
    /// Flat arena; `nodes[root]` is the root object, children by index.
    pub nodes: Vec<EfStoredObject>,
    pub root: usize,
}

/// Slack between the timeline ints and the view-mode source: `byte2 + int3 +
/// byte3`, exactly as the wiki says. Measured 6 in **75,450 of 75,450** nodes
/// across the 3,738-file corpus.
///
/// This used to try 7/5/8/4/9 as well, on the strength of a comment claiming
/// "real files also show 7" — no such file exists in the corpus. That search
/// did not fail closed: it accepted the first pad whose following bytes merely
/// *looked* like a view-mode source, so a genuinely different layout would
/// have been silently mis-parsed instead of producing a loggable error. If a
/// non-6 file ever turns up, add its size back with a citation.
const TAIL_PAD_LEN: usize = 6;

/// True if `rd` at its current position holds a plausible view-mode source.
fn view_source_plausible(rd: &Rd) -> bool {
    let data = rd.data;
    let pos = rd.pos;
    let Some(&has_data) = data.get(pos) else {
        return false;
    };
    // The view-mode source always carries data: requiring `hasData == 1`
    // still parses all 75,450 nodes of the 3,738-file corpus, so the former
    // `hasData == 0` branch never fired.
    if has_data != 1 {
        return false;
    }
    let Some(len_bytes) = data.get(pos + 1..pos + 5) else {
        return false;
    };
    let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
    let Some(name) = data.get(pos + 5..pos + 5 + len) else {
        return false;
    };
    VIEW_NAMES.iter().any(|v| v.as_bytes() == name)
}

fn parse_timeline(rd: &mut Rd) -> Result<NodeTimeline, ParseError> {
    let byte0 = rd.u8("timeline.byte0")?;
    let byte1 = rd.u8("timeline.byte1")?;
    let ints = [
        rd.u32("timeline.int0")?,
        rd.u32("timeline.int1")?,
        rd.u32("timeline.int2")?,
    ];
    let base = rd.pos();
    rd.set_pos(base + TAIL_PAD_LEN);
    if rd.remaining() == 0 || !view_source_plausible(rd) {
        rd.set_pos(base);
        return Err(rd.err("no plausible view-mode source after timeline tail"));
    }
    let pad = rd.data[base..base + TAIL_PAD_LEN].to_vec();
    Ok(NodeTimeline {
        byte0,
        byte1,
        ints,
        pad,
    })
}

fn parse_object(
    rd: &mut Rd,
    nodes: &mut Vec<EfStoredObject>,
    depth: usize,
) -> Result<usize, ParseError> {
    if depth > MAX_DEPTH {
        return Err(rd.err("effect tree too deep"));
    }

    let data_offset = rd.u32("EFStoredObject.dataOffset")?;
    let name = rd.string("EFStoredObject.name")?;

    let controller_count = rd.u32("controllerCount")?;
    if controller_count > 64 {
        return Err(rd.err(format!("implausible controller count {controller_count}")));
    }
    let mut controllers = Vec::with_capacity(controller_count as usize);
    for _ in 0..controller_count {
        let ctrl_name = rd.string("controllerName")?;
        controllers.push(parse_controller(rd, &ctrl_name)?);
    }

    let program_len = rd.u32("EEGlobalData.programLength")?;
    let param_count = rd.u32("EEGlobalData.parameterCount")?;
    if param_count > 64 {
        return Err(rd.err(format!("implausible global parameter count {param_count}")));
    }
    let mut global_params = Vec::with_capacity(param_count as usize);
    for _ in 0..param_count {
        let param_name = rd.string("EEGlobalData.parameterName")?;
        let param = parse_global_parameter(rd, &param_name)?;
        global_params.push((param_name, param));
    }

    let _empty0 = parse_source_list(rd, "emptySourceList0")?;
    let emitters = parse_source_list(rd, "emitterSourceList")?;
    let _empty2 = parse_source_list(rd, "emptySourceList2")?;
    let lifetime = parse_source_data(rd)?;
    let programs = parse_source_list(rd, "programSourceList")?;

    let timeline = parse_timeline(rd)?;

    let view_source = parse_source_data(rd)?;
    let view_mode = view_source
        .as_ref()
        .and_then(|s| ViewMode::from_name(s.command.name()))
        .unwrap_or_default();

    let resource = parse_resource(rd)?;

    let render_source = parse_source_data(rd)?;
    let render_shape = render_source
        .as_ref()
        .and_then(|s| RenderShape::from_name(s.command.name()))
        .unwrap_or_default();

    let _empty3 = parse_source_list(rd, "emptySourceList3")?;
    let decorations = parse_source_list(rd, "renderSourceList")?;

    let child_count = rd.u32("childObjectCount")?;
    if child_count > MAX_CHILDREN {
        return Err(rd.err(format!("implausible child count {child_count}")));
    }
    let mut children = Vec::with_capacity(child_count as usize);
    for _ in 0..child_count {
        children.push(parse_object(rd, nodes, depth + 1)?);
    }

    nodes.push(EfStoredObject {
        data_offset,
        name,
        controllers,
        program_len,
        global_params,
        emitters,
        lifetime,
        programs,
        timeline,
        view_mode,
        resource,
        render_shape,
        decorations,
        children,
    });
    Ok(nodes.len() - 1)
}

pub fn parse_efp(data: &[u8]) -> Result<EfStoredEffect, ParseError> {
    let mut rd = Rd::new(data);

    let sig = rd.take(8, "signature")?;
    if sig != b"JMXVEFF " {
        return Err(rd.err(format!("bad signature {:?}", String::from_utf8_lossy(sig))));
    }
    let version_bytes = rd.take(4, "version")?;
    let version_str: String = version_bytes.iter().map(|&b| b as char).collect();

    let mut root_scale = 1.0;
    let mut v13_ints = [0i32; 3];
    match version_str.as_str() {
        // "0010" is a single corpus file (skill/china/water_hide_keep_a.efp)
        // that parses byte-exact under the 0011 layout. The accept is not
        // fail-open on unseen "0010" variants: every record is range-checked
        // and the parse must consume the file exactly (see the trailing-byte
        // check below), so a structurally different file still errors out.
        "0010" | "0011" => {}
        "0012" => root_scale = rd.f32("rootScale")?,
        "0013" => {
            root_scale = rd.f32("rootScale")?;
            v13_ints = [rd.i32("v13Int0")?, rd.i32("v13Int1")?, rd.i32("v13Int2")?];
        }
        other => return Err(rd.err(format!("unsupported version \"{other}\""))),
    }

    let mut nodes = Vec::new();
    let root = parse_object(&mut rd, &mut nodes, 0)?;

    if rd.remaining() > 0 {
        return Err(rd.err(format!(
            "{} trailing bytes after root object",
            rd.remaining()
        )));
    }

    Ok(EfStoredEffect {
        version_str,
        root_scale,
        v13_ints,
        nodes,
        root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_sample_interpolates() {
        let b = EeBlend {
            begin: 0.0,
            end: 1.0,
            keys: vec![(0.0, 0.0f32), (1.0, 10.0)],
        };
        let lerp = |a: f32, c: f32, s: f32| a + (c - a) * s;
        assert_eq!(b.sample(-1.0, lerp), Some(0.0));
        assert_eq!(b.sample(0.5, lerp), Some(5.0));
        assert_eq!(b.sample(2.0, lerp), Some(10.0));
    }

    #[test]
    fn blend_sample_empty_is_none() {
        let b: EeBlend<f32> = EeBlend::default();
        assert_eq!(b.sample(0.5, |a, _, _| a), None);
    }

    #[test]
    fn argb_channel_order() {
        assert_eq!(argb_channels(0x80FF6E00), [0x80, 0xFF, 0x6E, 0x00]);
    }

    #[test]
    fn rejects_bad_signature() {
        assert!(parse_efp(b"JMXVBSK 0101").is_err());
    }

    #[test]
    fn mat4_reads_d3d_rows_as_glam_columns() {
        // A D3DXMatrixRotationY(+90°) laid out row-major on disk:
        // row0 = (cos, 0, -sin, 0), row1 = (0, 1, 0, 0),
        // row2 = (sin, 0, cos, 0),  row3 = (0, 0, 0, 1).
        // Under D3D's row-vector math, (0,0,1)·M = row2 = +X. The rows-as-
        // columns read must reproduce that under glam's M·v — a naive
        // "row-major fix" (.transpose()) would invert every stored rotation.
        #[rustfmt::skip]
        let rows: [f32; 16] = [
            0.0, 0.0, -1.0, 0.0,
            0.0, 1.0,  0.0, 0.0,
            1.0, 0.0,  0.0, 0.0,
            0.0, 0.0,  0.0, 1.0,
        ];
        let bytes: Vec<u8> = rows.iter().flat_map(|f| f.to_le_bytes()).collect();
        let mat = Rd::new(&bytes).mat4("yaw90").unwrap();
        let q = bevy::math::Quat::from_mat3(&bevy::math::Mat3::from_mat4(mat));
        let rotated = q * Vec3::Z;
        assert!(rotated.abs_diff_eq(Vec3::X, 1e-6), "{rotated:?}");
    }

    /// #291: `skill/china/water_hide_keep_a.efp` is the corpus' only `"0010"`
    /// file and parses byte-exact (9 nodes, 0 trailing bytes) under the 0011
    /// layout, so the version gate should let it through. Getting past the
    /// gate is what this asserts — the truncated body then fails on its own.
    #[test]
    fn version_0010_clears_the_version_gate() {
        let err = parse_efp(b"JMXVEFF 0010").unwrap_err();

        assert!(
            !err.what.contains("unsupported version"),
            "0010 must not be rejected as a version: {err}"
        );
    }

    /// The 7 corrupt `"0000"` files must still be rejected cleanly rather than
    /// waved through with the rest.
    #[test]
    fn an_unknown_version_is_still_rejected() {
        let err = parse_efp(b"JMXVEFF 0000").unwrap_err();

        assert!(err.what.contains("unsupported version"), "{err}");
    }
}

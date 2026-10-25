//! Configuration model: what a profile stores. Pure data with stable ids; serde mirrors
//! the JSON schema one to one (camelCase). Geometry lives elsewhere; nothing here holds
//! source drawing bytes.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::region::LoopRule;

/// Current profile schema version. Every change after deployment bumps it together with a
/// migration in `profile` (v1 was deployed on 2026-10-25; v2 adds the audit width).
pub const SCHEMA_VERSION: u32 = 2;

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(SourceId, "Stable id of a source-layer slot in a profile.");
id_type!(LayerId, "Stable id of a working layer.");
id_type!(PassId, "Stable id of a pattern pass.");
id_type!(GroupId, "Stable id of an output colour group.");

/// Exact RGB stroke colour of an output group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLUE: Rgb = Rgb(0, 0, 255);
    pub const RED: Rgb = Rgb(255, 0, 0);
    pub const BLACK: Rgb = Rgb(0, 0, 0);
    pub const GREEN: Rgb = Rgb(0, 160, 0);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SourceFormat {
    Dxf,
    Gerber,
    Svg,
    Pdf,
    Gds,
    #[serde(rename = "ipc2581")]
    Ipc2581,
}

/// Ordered matching rules binding a profile slot to imported source layers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MatchRule {
    /// Exact source layer name, optionally restricted to one input format.
    #[serde(rename_all = "camelCase")]
    LayerName {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<SourceFormat>,
        name: String,
    },
    /// Explicit user mapping to a layer of a specific file (by file name).
    #[serde(rename_all = "camelCase")]
    FileLayer { file: String, name: String },
    /// Case-insensitive glob (`*`, `?`) on the file name, optionally restricted to one
    /// format; meant for one-layer-per-file formats such as Gerber (`*.gtl`, `*-F_Cu.gbr`).
    /// Matches every non-empty layer of the file, so it is ambiguous on multi-layer files.
    #[serde(rename_all = "camelCase")]
    FileName {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<SourceFormat>,
        pattern: String,
    },
    /// Case-insensitive glob on the layer's declared function (Gerber X2 `TF.FileFunction`
    /// such as `Copper,L1,Top`, IPC-2581 `layerFunction`), independent of names.
    #[serde(rename_all = "camelCase")]
    Function { pattern: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceBinding {
    pub id: SourceId,
    pub name: String,
    /// A missing required binding blocks every output depending on it.
    #[serde(default)]
    pub required: bool,
    pub rules: Vec<MatchRule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GeometryKind {
    /// Filled planar area with holes.
    Region,
    /// Centrelines, open or closed; never implicitly an area.
    Linework,
    /// A bounded domain derived from a reference set by an [`EnvelopeProvider`]; a region
    /// meant to be subtracted from (negative artwork) or clipped against. Reference-only
    /// by default: it never marks unless a pass is added deliberately.
    Envelope,
}

/// How an envelope layer turns its reference set (group A) into a bounded region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "provider", rename_all = "camelCase")]
pub enum EnvelopeProvider {
    /// The filled interior of the reference region itself (holes included): a named
    /// boundary such as a mechanical outline.
    NamedBoundary,
    /// Axis-aligned bounding box of the reference set, grown by a signed margin.
    #[serde(rename_all = "camelCase")]
    BoundingBox { margin_mm: f64 },
    /// Smallest enclosing circle of the reference set, grown by a radial margin.
    #[serde(rename_all = "camelCase")]
    BoundingCircle { margin_mm: f64 },
    /// `buffer(reference_region, distance)` with the given corner joins (round by default).
    #[serde(rename_all = "camelCase")]
    Buffer {
        distance_mm: f64,
        #[serde(default, skip_serializing_if = "JoinStyle::is_round")]
        join: JoinStyle,
    },
    /// `buffer(convexHull(reference), distance)` with the given corner joins.
    #[serde(rename_all = "camelCase")]
    ConvexHull {
        distance_mm: f64,
        #[serde(default, skip_serializing_if = "JoinStyle::is_round")]
        join: JoinStyle,
    },
}

/// How an offset turns a corner it moves away from: an exact arc around the corner (round),
/// the two offset edges extended until they meet (miter, bevelled when the tip would lie
/// farther than `limit` × distance from the corner), or a straight chord (bevel).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JoinStyle {
    #[default]
    Round,
    Miter {
        #[serde(default = "default_miter_limit")]
        limit: f64,
    },
    Bevel,
}

fn default_miter_limit() -> f64 {
    4.0
}

impl JoinStyle {
    pub fn is_round(&self) -> bool {
        matches!(self, JoinStyle::Round)
    }
}

/// How a layer combines its two operand groups: `union(inputs) op union(operands)`.
/// For `Union` both groups are simply merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LayerOp {
    Union,
    Intersection,
    Subtraction,
    Xor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Operand {
    Source(SourceId),
    Layer(LayerId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LoopRuleCfg {
    EvenOdd,
    NonZero,
    MergeThenNest,
}

impl From<LoopRuleCfg> for LoopRule {
    fn from(v: LoopRuleCfg) -> Self {
        match v {
            LoopRuleCfg::EvenOdd => LoopRule::EvenOdd,
            LoopRuleCfg::NonZero => LoopRule::NonZero,
            LoopRuleCfg::MergeThenNest => LoopRule::MergeThenNest,
        }
    }
}

/// How open source linework contributes to a region layer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StrokeWidth {
    /// Open paths are an error in a region layer.
    None,
    /// Use the width the source carried (DXF polyline widths); widthless paths are errors.
    Original,
    /// Expand every open path with this width in mm.
    #[serde(rename_all = "camelCase")]
    Fixed { mm: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Interpretation {
    #[serde(default = "default_loop_rule")]
    pub loop_rule: LoopRuleCfg,
    #[serde(default = "default_stroke_width")]
    pub stroke_width: StrokeWidth,
}

fn default_loop_rule() -> LoopRuleCfg {
    LoopRuleCfg::MergeThenNest
}

fn default_stroke_width() -> StrokeWidth {
    StrokeWidth::Original
}

impl Default for Interpretation {
    fn default() -> Self {
        Self {
            loop_rule: default_loop_rule(),
            stroke_width: default_stroke_width(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Geometric path-generation strategy. Creates geometry, never machine settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Pattern {
    /// Source centrelines as they are (region layers contribute their boundaries).
    #[serde(rename_all = "camelCase")]
    Passthrough {
        /// Drop vertices that deviate less than this from the path (mm); absent keeps all.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        simplify_mm: Option<f64>,
        #[serde(default)]
        reverse: bool,
    },
    /// Iterative inward offsets from the boundary; rings stay closed, collapsed cores get
    /// an open centreline when `gap_fill` is on.
    #[serde(rename_all = "camelCase")]
    ContourFill {
        /// Centreline distance between rings (mm).
        pitch_mm: f64,
        /// Distance of the first ring from the boundary; default `pitch / 2`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_offset_mm: Option<f64>,
        #[serde(default = "default_true")]
        gap_fill: bool,
        /// How far beyond the innermost ring the centreline gap fill starts: the swept
        /// half-width credited to the last ring. Default `pitch / 2`. Larger values keep the
        /// gap-fill centrelines farther from the last ring (less double exposure); smaller
        /// values fill narrower cores.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_spacing_mm: Option<f64>,
        /// Simplifying tolerance for the rings' straight runs (mm, Douglas-Peucker; arcs are
        /// kept exactly); absent keeps every vertex. Clamped to a quarter of the pitch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        simplify_mm: Option<f64>,
    },
    /// Trace the region boundary (outers and holes) at a geometric offset, optionally as
    /// several rings. Kerf compensation is a user-entered distance, applied outward from the
    /// filled area (outers grow, holes shrink), never assumed.
    #[serde(rename_all = "camelCase")]
    BoundaryTrace {
        #[serde(default)]
        side: OffsetSide,
        /// Distance from the boundary in the direction of `side` (mm).
        #[serde(default)]
        offset_mm: f64,
        /// Beam/kerf width to compensate (mm); half of it is applied outward.
        #[serde(default)]
        kerf_mm: f64,
        /// Number of rings; rings after the first step further in the direction of `side`
        /// (inward for `on`).
        #[serde(default = "default_one")]
        rings: u32,
        #[serde(default)]
        ring_spacing_mm: f64,
        /// Reverse every path's direction.
        #[serde(default)]
        reverse: bool,
        /// Emit holes before the outer contour that contains them (cutting order).
        #[serde(default = "default_true")]
        inner_first: bool,
        /// Corner joins of the offset rings (round, miter, bevel).
        #[serde(default, skip_serializing_if = "JoinStyle::is_round")]
        join: JoinStyle,
    },
    /// Equally spaced parallel scanlines clipped to the region and its holes.
    #[serde(rename_all = "camelCase")]
    Hatch {
        pitch_mm: f64,
        /// Measured in document coordinates (degrees, counter-clockwise from +x).
        #[serde(default)]
        angle_deg: f64,
        /// Lines lie at `phase + k * pitch` across the rotated frame: a global lattice, so
        /// adjacent layers with the same parameters stay aligned.
        #[serde(default)]
        phase_mm: f64,
        /// Alternate row direction (bidirectional) instead of unidirectional.
        #[serde(default = "default_true")]
        serpentine: bool,
        /// Shrink the region by this much before hatching (edge coverage control).
        #[serde(default)]
        inset_mm: f64,
    },
    /// Independent hatches at several angles.
    #[serde(rename_all = "camelCase")]
    CrossHatch {
        pitch_mm: f64,
        angles_deg: Vec<f64>,
        #[serde(default)]
        phase_mm: f64,
        #[serde(default = "default_true")]
        serpentine: bool,
        #[serde(default)]
        inset_mm: f64,
    },
    /// Perimeter shells followed by a hatch of the remaining interior.
    #[serde(rename_all = "camelCase")]
    BoundaryHatch {
        #[serde(default = "default_one")]
        shells: u32,
        shell_pitch_mm: f64,
        /// Gap between the innermost shell and the hatch area (mm).
        #[serde(default)]
        inset_mm: f64,
        pitch_mm: f64,
        #[serde(default = "default_angles")]
        angles_deg: Vec<f64>,
        #[serde(default)]
        phase_mm: f64,
        #[serde(default = "default_true")]
        serpentine: bool,
    },
}

fn default_one() -> u32 {
    1
}

fn default_angles() -> Vec<f64> {
    vec![0.0]
}

/// Which side of the boundary a trace runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum OffsetSide {
    Inside,
    #[default]
    On,
    Outside,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pass {
    pub id: PassId,
    pub name: String,
    pub pattern: Pattern,
    pub group: GroupId,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkingLayer {
    pub id: LayerId,
    pub name: String,
    pub kind: GeometryKind,
    /// Group A, combined by union.
    pub inputs: Vec<Operand>,
    /// Group B, combined by union; the right-hand side of `op`. Unused for `Union`.
    #[serde(default)]
    pub operands: Vec<Operand>,
    pub op: LayerOp,
    /// Required when `kind` is `Envelope`; ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<EnvelopeProvider>,
    #[serde(default)]
    pub interpretation: Interpretation,
    #[serde(default)]
    pub passes: Vec<Pass>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Visible and usable as an operand, but never emits marks of its own.
    #[serde(default)]
    pub reference_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutputGroup {
    pub id: GroupId,
    pub name: String,
    pub color: Rgb,
    /// PDF stroke width override (mm); absent uses the export default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width_mm: Option<f64>,
    #[serde(default = "default_true")]
    pub export: bool,
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Per-group path-order mode; absent follows the profile's optimizer mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_mode: Option<OptimizerMode>,
    /// Effective marking width (mm) the coverage audit assumes each path sweeps: a
    /// geometric surrogate set by the user, not an exposure model. Absent turns the audit
    /// off for the group's passes. Schema v2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_width_mm: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportSettings {
    /// Cosmetic PDF stroke width (mm). 0.0254 mm = 1 mil, the validated style.
    pub stroke_width_mm: f64,
    /// Page margin around the geometry bounds (mm).
    pub margin_mm: f64,
    /// One PDF per output group instead of one colour-separated file.
    pub separate_files: bool,
    /// Largest deviation of the Bézier curves the PDF export writes for arcs (mm). DXF and
    /// SVG exports write arcs exactly. (The name is kept for profile compatibility.)
    pub flatten_tolerance_mm: f64,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            stroke_width_mm: 0.0254,
            margin_mm: 5.0,
            separate_files: false,
            flatten_tolerance_mm: 0.001,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum OptimizerMode {
    AsGenerated,
    /// The default for new profiles (owner decision, 2026-10-25).
    #[default]
    MinimumJump,
    BleedAware,
}

/// Path-order planning within each output group (spec §7). Everything here is geometric
/// prediction under the stated assumption of straight beam-off jumps between emitted path
/// endpoints; nothing is a machine setting or a calibrated dose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptimizerSettings {
    #[serde(default)]
    pub mode: OptimizerMode,
    /// Assumed width of a jump's influence footprint (mm): the geometric surrogate used for
    /// the bleed penalty and the heatmap.
    #[serde(default = "default_footprint")]
    pub footprint_mm: f64,
    /// Weight of the bleed penalty (squared footprint overlap over protected regions, in
    /// mm) relative to jump length (mm) in `bleedAware` mode.
    #[serde(default = "default_one_f64")]
    pub bleed_weight: f64,
    /// Working layers whose regions are protected (keep-clear): the bleed penalty and the
    /// heatmap are evaluated over their union. Empty means no protected region.
    #[serde(default)]
    pub protected: Vec<LayerId>,
    /// Raster cell size for the footprint model (mm); coarsened automatically for very
    /// large groups.
    #[serde(default = "default_cell")]
    pub cell_mm: f64,
    /// Assumed position of the beam before the first path, if the user knows it. Never
    /// invented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_mm: Option<[f64; 2]>,
    /// Assumed position the beam returns to after the last path, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_mm: Option<[f64; 2]>,
    /// Upper bound on local-improvement sweeps (determinism and bounded time).
    #[serde(default = "default_sweeps")]
    pub max_sweeps: u32,
}

fn default_footprint() -> f64 {
    0.1
}

fn default_one_f64() -> f64 {
    1.0
}

fn default_cell() -> f64 {
    0.05
}

fn default_sweeps() -> u32 {
    20
}

impl Default for OptimizerSettings {
    fn default() -> Self {
        Self {
            mode: OptimizerMode::default(),
            footprint_mm: default_footprint(),
            bleed_weight: default_one_f64(),
            protected: Vec::new(),
            cell_mm: default_cell(),
            start_mm: None,
            end_mm: None,
            max_sweeps: default_sweeps(),
        }
    }
}

/// A geometry-preparation profile: everything needed to turn matching source layers into
/// output groups on a clean browser. No laser parameters, no drawing data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub schema_version: u32,
    pub profile_id: String,
    pub name: String,
    #[serde(default)]
    pub source_bindings: Vec<SourceBinding>,
    #[serde(default)]
    pub working_layers: Vec<WorkingLayer>,
    #[serde(default)]
    pub output_groups: Vec<OutputGroup>,
    #[serde(default)]
    pub export: ExportSettings,
    #[serde(default)]
    pub optimizer: OptimizerSettings,
}

impl Profile {
    pub fn empty(profile_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            profile_id: profile_id.into(),
            name: name.into(),
            source_bindings: Vec::new(),
            working_layers: Vec::new(),
            output_groups: Vec::new(),
            export: ExportSettings::default(),
            optimizer: OptimizerSettings::default(),
        }
    }

    pub fn layer(&self, id: &LayerId) -> Option<&WorkingLayer> {
        self.working_layers.iter().find(|l| &l.id == id)
    }

    pub fn group(&self, id: &GroupId) -> Option<&OutputGroup> {
        self.output_groups.iter().find(|g| &g.id == id)
    }

    pub fn binding(&self, id: &SourceId) -> Option<&SourceBinding> {
        self.source_bindings.iter().find(|b| &b.id == id)
    }
}

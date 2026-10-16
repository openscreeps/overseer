// The overlay over the world (appended to common.wgsl): marks (owners' badges, portals, the
// hovered room's outline), arcs between linked portals and labels, on the ground and sized
// in CSS pixels; composed premultiplied, without depth. `overlay.rs` has the rules.

struct Overlay {
    // x: CSS pixels per room at the target, y: tiles per CSS pixel, zw: canvas size (CSS
    // pixels).
    view: vec4<f32>,
    // Opacity of labels by class: x sectors, y rooms, z shard names; w: clip-space w of the
    // camera's target (rooms elsewhere are `w / their w` as large: perspective).
    labels: vec4<f32>,
    // Opacity of x badges, y portal runs (marks and arcs), z portal tiles, w the hover
    // outline.
    marks: vec4<f32>,
    // Detail scenes: xy their room's centre (world), z their opacity.
    details: array<vec4<f32>, 4>,
}

@group(1) @binding(0) var<uniform> overlay: Overlay;
@group(2) @binding(0) var glyph_atlas: texture_2d<f32>;
@group(2) @binding(1) var glyph_sampler: sampler;
@group(3) @binding(0) var badge_atlas: texture_2d_array<f32>;
@group(3) @binding(1) var badge_sampler: sampler;

// Outside the clip volume: instances faded out cost no fragments.
const HIDDEN: vec4<f32> = vec4<f32>(2.0, 2.0, 2.0, 1.0);
// Height of the overlay above the ground.
const LIFT: f32 = 0.05;
// Segments of an arc, and half its width in CSS pixels.
const ARC_SEGMENTS: u32 = 24u;
const ARC_HALF_WIDTH: f32 = 0.75;

fn overlay_corner(index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    return corners[index];
}

fn on_ground(point: vec2<f32>) -> vec4<f32> {
    return globals.view_projection * vec4<f32>(point.x, LIFT, point.y, 1.0);
}

// A clip-space position moved by CSS pixels (y down).
fn nudge(clip: vec4<f32>, px: vec2<f32>) -> vec4<f32> {
    let ndc = vec2<f32>(px.x, -px.y) * 2.0 / max(overlay.view.zw, vec2<f32>(1.0));
    return vec4<f32>(clip.xy + ndc * clip.w, clip.zw);
}

fn coverage(distance: f32, px: f32) -> f32 {
    return clamp(0.5 - distance / max(px, 1e-5), 0.0, 1.0);
}

// Linear premultiplied colour of sRGB `color` at `opacity`.
fn premultiplied(color: vec4<f32>, opacity: f32) -> vec4<f32> {
    let alpha = color.a * opacity;
    return vec4<f32>(srgb_to_linear(color.rgb) * alpha, alpha);
}

// Opacity of the detail scene of the room centred at `centre` (0 without one).
fn detail_at(centre: vec2<f32>) -> f32 {
    var opacity = 0.0;
    for (var index = 0u; index < 4u; index++) {
        let detail = overlay.details[index];
        if all(abs(detail.xy - centre) < vec2<f32>(1.0)) {
            opacity = max(opacity, detail.z);
        }
    }
    return opacity;
}

struct MarkIn {
    @location(0) centre: vec2<f32>,
    // Radius in tiles, least and greatest radius in CSS pixels.
    @location(1) radius: vec3<f32>,
    @location(2) uv: vec4<f32>,
    @location(3) color: vec4<f32>,
    // Kind (`mesh::mark`) in the low byte, badge atlas layer above.
    @location(4) kind: u32,
}

struct MarkOut {
    @builtin(position) position: vec4<f32>,
    // Tiles from the centre.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) uv: vec4<f32>,
    @location(3) @interpolate(flat) kind: u32,
    @location(4) @interpolate(flat) radius: f32,
}

// The half of the spine of a run's capsule (tiles; zero for a disc): portal runs only.
fn capsule_half(kind: u32, uv: vec4<f32>) -> vec2<f32> {
    return select(vec2<f32>(0.0), uv.xy, kind == 2u || kind == 3u);
}

@vertex
fn mark_vertex(@builtin(vertex_index) index: u32, in: MarkIn) -> MarkOut {
    var out: MarkOut;
    let tiles_per_px = overlay.view.y;
    let radius = clamp(
        in.radius.x,
        in.radius.y * tiles_per_px,
        max(in.radius.z, in.radius.y) * tiles_per_px,
    );
    let kind = in.kind & 255u;
    var opacity = overlay.marks.w;
    if kind <= 1u {
        opacity = overlay.marks.x * (1.0 - detail_at(in.centre));
    } else if kind == 2u || kind == 3u {
        opacity = overlay.marks.y;
    } else if kind >= 5u {
        opacity = overlay.marks.z;
    }
    out.color = premultiplied(in.color, opacity);
    let extent = abs(capsule_half(kind, in.uv)) + vec2<f32>(radius + 2.0 * tiles_per_px);
    out.local = (overlay_corner(index) * 2.0 - vec2<f32>(1.0)) * extent;
    out.uv = in.uv;
    out.kind = in.kind;
    out.radius = radius;
    out.position = on_ground(in.centre + out.local);
    if out.color.a <= 0.0 {
        out.position = HIDDEN;
    }
    return out;
}

@fragment
fn mark_fragment(in: MarkOut) -> @location(0) vec4<f32> {
    // Tiles per pixel; the badge sampled in uniform control flow (portals ignore it).
    let px = max(length(fwidth(in.local)) * 0.7071, 1e-5);
    let unit = clamp(in.local / max(in.radius, 1e-5), vec2<f32>(-1.0), vec2<f32>(1.0));
    let badge = textureSample(
        badge_atlas,
        badge_sampler,
        in.uv.xy + (unit * 0.5 + vec2<f32>(0.5)) * in.uv.zw,
        i32(in.kind >> 8u),
    );
    let kind = in.kind & 255u;
    let opacity = in.color.a;
    let tint = select(in.color.rgb / opacity, vec3<f32>(0.0), opacity <= 0.0);
    let d = length(in.local) - in.radius;
    if kind <= 1u {
        // The badge within a dark rim about a pixel wide.
        let rim = vec4<f32>(0.0, 0.0, 0.0, 0.6) * coverage(d, px);
        let inner = badge * coverage(d + px, px);
        return (inner + rim * (1.0 - inner.a)) * opacity;
    }
    if kind == 4u {
        // A line inside the room's border, 1.5 pixels wide.
        let q = abs(in.local);
        let edge = max(q.x, q.y) - in.radius;
        let line = coverage(abs(edge + 1.25 * px) - 0.75 * px, px);
        return vec4<f32>(tint, 1.0) * opacity * line;
    }
    // Portals: a translucent disc (or capsule along a run) in a ring, a dot (a line along
    // a run) in the middle; another shard's a second ring. Tiles have no dot: the tile
    // shows its portal.
    let half = capsule_half(kind, in.uv);
    let along = clamp(dot(in.local, half) / max(dot(half, half), 1e-6), -1.0, 1.0);
    let spine = length(in.local - half * along);
    let edge = spine - in.radius;
    var acc = vec4<f32>(tint, 1.0) * 0.22 * coverage(edge, px);
    var ring = coverage(abs(edge + 1.25 * px) - 0.75 * px, px);
    if kind == 3u || kind == 6u {
        ring = max(ring, coverage(abs(edge + in.radius * 0.45) - 0.6 * px, px));
    }
    let middle = coverage(spine - max(in.radius * 0.18, 1.2 * px), px);
    let line = max(ring, select(middle, 0.0, kind >= 5u));
    acc = acc * (1.0 - line) + vec4<f32>(tint, 1.0) * line;
    return acc * opacity;
}

struct ArcIn {
    @location(0) a: vec2<f32>,
    @location(1) b: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) bend: f32,
}

struct ArcOut {
    @builtin(position) position: vec4<f32>,
    // CSS pixels across the line from its middle.
    @location(0) across: f32,
    @location(1) @interpolate(flat) color: vec4<f32>,
}

fn arc_point(a: vec2<f32>, control: vec2<f32>, b: vec2<f32>, t: f32) -> vec2<f32> {
    let s = 1.0 - t;
    return s * s * a + 2.0 * s * t * control + t * t * b;
}

// An arc is a ribbon of `ARC_SEGMENTS` quads along a quadratic curve, its width in CSS
// pixels across the curve's direction on screen; neighbouring quads share their edges.
@vertex
fn arc_vertex(@builtin(vertex_index) index: u32, in: ArcIn) -> ArcOut {
    var out: ArcOut;
    let corner = overlay_corner(index % 6u);
    let t = (f32(index / 6u) + corner.x) / f32(ARC_SEGMENTS);
    let side = corner.y * 2.0 - 1.0;
    let chord = in.b - in.a;
    let control = (in.a + in.b) * 0.5 + vec2<f32>(chord.y, -chord.x) * in.bend;
    let point = arc_point(in.a, control, in.b, t);
    let tangent = 2.0 * (1.0 - t) * (control - in.a) + 2.0 * t * (in.b - control);
    let here = on_ground(point);
    let step = normalize(tangent + vec2<f32>(1e-6, 0.0)) * overlay.view.y * 4.0;
    let ahead = on_ground(point + step);
    // The direction on screen in CSS pixels (y up), and its normal.
    let direction = (ahead.xy / ahead.w - here.xy / here.w) * overlay.view.zw;
    let normal = normalize(vec2<f32>(-direction.y, direction.x) + vec2<f32>(0.0, 1e-6));
    let reach = ARC_HALF_WIDTH + 1.0;
    out.across = side * reach;
    out.position = nudge(here, vec2<f32>(normal.x, -normal.y) * out.across);
    out.color = premultiplied(in.color, overlay.marks.y);
    if out.color.a <= 0.0 || here.w <= 0.0 {
        out.position = HIDDEN;
    }
    return out;
}

@fragment
fn arc_fragment(in: ArcOut) -> @location(0) vec4<f32> {
    return in.color * clamp(ARC_HALF_WIDTH + 0.5 - abs(in.across), 0.0, 1.0);
}

struct LabelIn {
    @location(0) anchor: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) uv: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) halo: vec4<f32>,
    @location(5) halo_width: f32,
    @location(6) appear: f32,
    @location(7) kind: u32,
}

struct LabelOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) halo: vec4<f32>,
    @location(3) @interpolate(flat) halo_width: f32,
}

@vertex
fn label_vertex(@builtin(vertex_index) index: u32, in: LabelIn) -> LabelOut {
    var out: LabelOut;
    var class_opacity = overlay.labels.x;
    if in.kind == 1u {
        class_opacity = overlay.labels.y;
    } else if in.kind == 2u {
        class_opacity = overlay.labels.z;
    }
    let anchor = on_ground(in.anchor);
    // Rooms where the label is are this many pixels wide (smaller far away in perspective).
    let px_per_room = overlay.view.x * overlay.labels.w / max(anchor.w, 1e-6);
    let opacity = class_opacity * smoothstep(in.appear, in.appear * 1.15 + 0.01, px_per_room);
    let corner = overlay_corner(index);
    out.position = nudge(anchor, in.rect.xy + corner * in.rect.zw);
    out.uv = in.uv.xy + corner * in.uv.zw;
    out.color = premultiplied(in.color, opacity);
    out.halo = premultiplied(in.halo, opacity);
    out.halo_width = in.halo_width;
    if opacity <= 0.0 || anchor.w <= 0.0 {
        out.position = HIDDEN;
    }
    return out;
}

@fragment
fn label_fragment(in: LabelOut) -> @location(0) vec4<f32> {
    let value = textureSample(glyph_atlas, glyph_sampler, in.uv).r;
    let w = max(fwidth(value) * 0.7, 1e-3);
    let glyph = smoothstep(0.5 - w, 0.5 + w, value);
    let edge = 0.5 - in.halo_width;
    let halo = smoothstep(edge - 2.0 * w, edge + w, value) * (1.0 - glyph);
    return in.color * glyph + in.halo * halo;
}

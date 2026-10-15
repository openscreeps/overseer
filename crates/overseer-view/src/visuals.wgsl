// Room visuals of bots (appended to the detail module): lines, circles and rectangles as
// signed distance fields, polygon fills as triangles, text from the glyph atlas's distance
// field; flat just above the ground, composed premultiplied.

@group(2) @binding(0) var atlas: texture_2d<f32>;
@group(2) @binding(1) var atlas_sampler: sampler;

// Height of visuals above the ground.
const VISUAL_LIFT: f32 = 0.03;

fn visual_point(local: vec2<f32>) -> vec4<f32> {
    let world = locals.offset.xyz + vec3<f32>(local.x, VISUAL_LIFT, local.y);
    return globals.view_projection * vec4<f32>(world, 1.0);
}

fn visual_color(color: vec4<f32>) -> vec4<f32> {
    let alpha = color.a * locals.offset.w;
    return vec4<f32>(srgb_to_linear(color.rgb) * alpha, alpha);
}

struct PrimIn {
    @location(0) a: vec2<f32>,
    @location(1) b: vec2<f32>,
    @location(2) size: f32,
    @location(3) stroke_width: f32,
    @location(4) fill: vec4<f32>,
    @location(5) stroke: vec4<f32>,
    @location(6) kind: u32,
}

struct PrimOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) a: vec2<f32>,
    @location(2) @interpolate(flat) b: vec2<f32>,
    @location(3) @interpolate(flat) size: f32,
    @location(4) @interpolate(flat) stroke_width: f32,
    @location(5) @interpolate(flat) fill: vec4<f32>,
    @location(6) @interpolate(flat) stroke: vec4<f32>,
    @location(7) @interpolate(flat) kind: u32,
}

fn visual_corner(index: u32) -> vec2<f32> {
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

@vertex
fn prim_vertex(@builtin(vertex_index) index: u32, in: PrimIn) -> PrimOut {
    var out: PrimOut;
    let corner = visual_corner(index);
    let kind = in.kind & 255u;
    // Room for the outline and antialiasing.
    let margin = in.stroke_width * 0.5 + 0.08;
    var low: vec2<f32>;
    var high: vec2<f32>;
    if kind == 0u {
        let reach = in.size * 0.5 + 0.08;
        low = min(in.a, in.b) - vec2<f32>(reach);
        high = max(in.a, in.b) + vec2<f32>(reach);
    } else if kind == 1u {
        low = in.a - vec2<f32>(in.size + margin);
        high = in.a + vec2<f32>(in.size + margin);
    } else {
        low = in.a - vec2<f32>(margin);
        high = in.b + vec2<f32>(margin);
    }
    out.local = mix(low, high, corner);
    out.position = visual_point(out.local);
    out.a = in.a;
    out.b = in.b;
    out.size = in.size;
    out.stroke_width = in.stroke_width;
    out.fill = visual_color(in.fill);
    out.stroke = visual_color(in.stroke);
    out.kind = in.kind;
    return out;
}

// Coverage of a dash pattern at `s` (tiles along the outline) for lines `width` wide.
fn dashes(s: f32, width: f32, style: u32, px: f32) -> f32 {
    if style == 0u {
        return 1.0;
    }
    let w = max(width, 0.05);
    var on = 3.0 * w;
    var period = 5.0 * w;
    if style == 2u {
        on = w;
        period = 2.5 * w;
    }
    let phase = wrap(s, period);
    let d = max(phase - on, -phase);
    return clamp(0.5 - d / max(px, 1e-4), 0.0, 1.0);
}

@fragment
fn prim_fragment(in: PrimOut) -> @location(0) vec4<f32> {
    let p = in.local;
    let px = max(length(fwidth(p)) * 0.75, 1e-4);
    let kind = in.kind & 255u;
    let style = (in.kind >> 8u) & 255u;
    var acc = vec4<f32>(0.0);
    if kind == 0u {
        // A line: a segment with round caps, its colour in `fill`.
        let ba = in.b - in.a;
        let h = clamp(dot(p - in.a, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
        let d = length(p - in.a - ba * h) - in.size * 0.5;
        let s = dot(p - in.a, normalize(ba + vec2<f32>(1e-6, 0.0)));
        acc = in.fill * cover(d, px) * dashes(s, in.size, style, px);
        return acc;
    }
    var sd: f32;
    var s: f32;
    if kind == 1u {
        sd = length(p - in.a) - in.size;
        s = (atan2(p.y - in.a.y, p.x - in.a.x) + PI) * in.size;
    } else {
        let centre = (in.a + in.b) * 0.5;
        let half = (in.b - in.a) * 0.5;
        sd = sd_box(p - centre, half, 0.0);
        // Along the outline, clockwise from the north-west corner.
        let q = p - in.a;
        let size = in.b - in.a;
        if abs(p.y - in.a.y) <= min(abs(p.x - in.a.x), abs(p.x - in.b.x)) {
            s = q.x;
        } else if abs(p.x - in.b.x) <= abs(p.y - in.b.y) {
            s = size.x + q.y;
        } else if abs(p.y - in.b.y) <= abs(p.x - in.a.x) {
            s = size.x + size.y + (size.x - q.x);
        } else {
            s = 2.0 * size.x + size.y + (size.y - q.y);
        }
    }
    acc = in.fill * cover(sd, px);
    if in.stroke_width > 0.0 {
        let outline = cover(abs(sd) - in.stroke_width * 0.5, px) * dashes(s, in.stroke_width, style, px);
        acc = acc * (1.0 - in.stroke.a * outline) + in.stroke * outline;
    }
    return acc;
}

struct FillOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn fill_vertex(@location(0) position: vec2<f32>, @location(1) color: vec4<f32>) -> FillOut {
    var out: FillOut;
    out.position = visual_point(position);
    out.color = visual_color(color);
    return out;
}

@fragment
fn fill_fragment(in: FillOut) -> @location(0) vec4<f32> {
    return in.color;
}

struct GlyphIn {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) stroke: vec4<f32>,
    @location(4) stroke_width: f32,
}

struct GlyphOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) stroke: vec4<f32>,
    @location(3) @interpolate(flat) stroke_width: f32,
}

@vertex
fn glyph_vertex(@builtin(vertex_index) index: u32, in: GlyphIn) -> GlyphOut {
    var out: GlyphOut;
    let corner = visual_corner(index);
    out.position = visual_point(in.rect.xy + corner * in.rect.zw);
    out.uv = in.uv.xy + corner * in.uv.zw;
    out.color = visual_color(in.color);
    out.stroke = visual_color(in.stroke);
    out.stroke_width = in.stroke_width;
    return out;
}

@fragment
fn glyph_fragment(in: GlyphOut) -> @location(0) vec4<f32> {
    let value = textureSample(atlas, atlas_sampler, in.uv).r;
    let w = max(fwidth(value) * 0.7, 1e-3);
    let glyph = smoothstep(0.5 - w, 0.5 + w, value);
    var acc = in.color * glyph;
    if in.stroke_width > 0.0 {
        let edge = 0.5 - in.stroke_width;
        let outline = smoothstep(edge - w, edge + w, value) * (1.0 - glyph);
        acc = acc + in.stroke * outline;
    }
    return acc;
}

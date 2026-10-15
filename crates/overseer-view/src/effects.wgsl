// Action effects (appended to the detail module): beams and bolts as ribbons facing the
// camera between two room-local points, bursts, flashes and pulses as rings on the ground;
// glowing, added to what is behind them.

struct EffectIn {
    @location(0) source: vec3<f32>,
    @location(1) progress: f32,
    @location(2) goal: vec3<f32>,
    @location(3) width: f32,
    @location(4) color: vec4<f32>,
    @location(5) look: u32,
}

struct EffectOut {
    @builtin(position) position: vec4<f32>,
    // Beams: tiles along the beam (from the source) and across; rings: tiles from the centre.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) span: f32,
    @location(2) @interpolate(flat) width: f32,
    @location(3) @interpolate(flat) progress: f32,
    @location(4) @interpolate(flat) look: u32,
    @location(5) color: vec3<f32>,
}

const BEAM: u32 = 0u;
const BOLT: u32 = 1u;
const BURST: u32 = 2u;
const FLASH: u32 = 3u;
const PULSE: u32 = 4u;

fn effect_corner(index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    return corners[index];
}

@vertex
fn effect_vertex(@builtin(vertex_index) index: u32, in: EffectIn) -> EffectOut {
    var out: EffectOut;
    let corner = effect_corner(index);
    let source = locals.offset.xyz + in.source;
    let goal = locals.offset.xyz + in.goal;
    out.width = in.width;
    out.progress = in.progress;
    out.look = in.look;
    out.color = srgb_to_linear(in.color.rgb) * in.color.a * locals.offset.w;
    var world: vec3<f32>;
    if in.look == BEAM || in.look == BOLT {
        let axis = goal - source;
        let span = max(length(axis), 1e-3);
        let along = axis / span;
        let middle = (source + goal) * 0.5;
        var across = cross(along, globals.eye.xyz - middle);
        if dot(across, across) < 1e-6 {
            across = cross(along, vec3<f32>(0.0, 1.0, 0.0));
        }
        across = normalize(across);
        let reach = in.width * 2.5;
        let t = corner.x * (span + 2.0 * reach) - reach;
        world = source + along * t + across * corner.y * reach;
        out.local = vec2<f32>(t, corner.y * reach);
        out.span = span;
    } else {
        // Rings lie on the ground around their centre.
        let centre = select(goal, source, in.look == PULSE);
        let reach = in.width * 1.25 + 0.1;
        let q = vec2<f32>(corner.x * 2.0 - 1.0, corner.y) * reach;
        world = centre + vec3<f32>(q.x, -0.3, q.y);
        out.local = q;
        out.span = 0.0;
    }
    out.position = globals.view_projection * vec4<f32>(world, 1.0);
    return out;
}

@fragment
fn effect_fragment(in: EffectOut) -> @location(0) vec4<f32> {
    let t = in.progress;
    let px = max(length(fwidth(in.local)), 1e-4);
    var intensity = 0.0;
    switch in.look {
        case 0u: {
            // Beam: a bright core and a soft glow, dashes flowing towards the goal.
            let along = clamp(in.local.x, 0.0, in.span);
            let d = length(vec2<f32>(in.local.x - along, in.local.y));
            let core = exp(-pow(d / max(in.width * 0.3, px), 2.0));
            let glow = exp(-pow(d / in.width, 2.0)) * 0.45;
            let flow = 0.65 + 0.35 * sin((in.local.x * 9.0 - globals.eye.w * 14.0));
            let envelope = smoothstep(0.0, 0.12, t) * (1.0 - smoothstep(0.6, 1.0, t));
            intensity = (core + glow) * flow * envelope;
        }
        case 1u: {
            // Bolt: a head flying to the goal with a fading tail, a spark on arrival.
            let head = in.span * (1.0 - pow(1.0 - min(t / 0.75, 1.0), 2.0));
            let behind = head - in.local.x;
            let d = length(vec2<f32>(max(abs(in.local.x - head), 0.0), in.local.y));
            let bolt = exp(-pow(d / (in.width * 0.6), 2.0));
            let tail = select(0.0, exp(-behind / 0.7) * exp(-pow(in.local.y / (in.width * 0.5), 2.0)) * 0.7, behind > 0.0 && in.local.x > 0.0);
            let flying = 1.0 - smoothstep(0.7, 0.8, t);
            let spark = exp(-pow(length(vec2<f32>(in.local.x - in.span, in.local.y)) / (in.width * (1.0 + 4.0 * t)), 2.0))
                * smoothstep(0.7, 0.78, t) * (1.0 - smoothstep(0.8, 1.0, t));
            intensity = (bolt + tail) * flying + spark;
        }
        case 2u: {
            // Burst: an expanding ring and a fading flash.
            let r = length(in.local);
            let radius = in.width * (0.25 + 0.75 * (1.0 - pow(1.0 - t, 3.0)));
            let ring = exp(-pow((r - radius) / max(0.05, px), 2.0));
            let flash = exp(-pow(r / (in.width * 0.6), 2.0)) * (1.0 - smoothstep(0.0, 0.5, t));
            intensity = (ring * 0.9 + flash) * (1.0 - smoothstep(0.55, 1.0, t));
        }
        case 3u: {
            // Flash: a ring tightening on the object.
            let r = length(in.local);
            let radius = in.width * (1.15 - 0.3 * t);
            let ring = exp(-pow((r - radius) / max(0.06, px), 2.0));
            let glow = exp(-pow(r / in.width, 2.0)) * 0.35;
            intensity = (ring + glow) * (1.0 - smoothstep(0.3, 1.0, t)) * smoothstep(0.0, 0.08, t);
        }
        default: {
            // Pulse: a ring spreading to the range.
            let r = length(in.local);
            let radius = in.width * (1.0 - pow(1.0 - t, 2.0));
            let ring = exp(-pow((r - radius) / max(0.12, px), 2.0));
            intensity = ring * (1.0 - smoothstep(0.5, 1.0, t)) * step(r, in.width + 0.2);
        }
    }
    return vec4<f32>(in.color * intensity, 0.0);
}

// Object tokens and their shadows (appended to common.wgsl, shader.wgsl and the badge atlas
// constants `BADGE_*`): every object is a square on top of its tile (or of its prism) whose
// fragment shader draws its emblem with signed distance fields, antialiased by the pixel
// footprint. Colours are composed premultiplied.

// The badge atlas (`overlay/badges.rs`): creeps and power creeps carry their owner's badge.
// Group 2 is the glyph atlas of the visuals in the same module.
@group(3) @binding(0) var badge_atlas: texture_2d_array<f32>;
@group(3) @binding(1) var badge_sampler: sampler;

struct TokenIn {
    @location(0) position: vec3<f32>,
    @location(1) size: f32,
    @location(2) kind: u32,
    @location(3) accent: vec4<f32>,
    @location(4) fill: vec4<f32>,
    @location(5) gauges: vec4<u32>,
    @location(6) parts_a: vec4<u32>,
    @location(7) parts_b: vec4<u32>,
    @location(8) fade: f32,
    @location(9) base: f32,
    @location(10) badge: u32,
}

struct TokenOut {
    @builtin(position) position: vec4<f32>,
    // Tiles from the token's centre (x east, y south).
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) kind: u32,
    @location(2) @interpolate(flat) size: f32,
    @location(3) accent: vec4<f32>,
    @location(4) fill: vec4<f32>,
    @location(5) @interpolate(flat) gauges: vec4<u32>,
    @location(6) @interpolate(flat) parts_a: vec4<u32>,
    @location(7) @interpolate(flat) parts_b: vec4<u32>,
    @location(8) @interpolate(flat) fade: f32,
    @location(9) @interpolate(flat) lift: f32,
    @location(10) @interpolate(flat) badge: u32,
}

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;
const NO_SHAPE: u32 = 255u;
const GHOST: u32 = 4u;
const SPAWNING: u32 = 1u;

// Corner `index` (0..6) of a unit quad, as two triangles.
fn token_corner(index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    return corners[index] * 2.0 - vec2<f32>(1.0);
}

fn token_out(in: TokenIn, corner: vec2<f32>, half: f32, world: vec3<f32>) -> TokenOut {
    var out: TokenOut;
    out.position = globals.view_projection * vec4<f32>(world, 1.0);
    out.local = corner * half;
    out.kind = in.kind;
    out.size = in.size;
    out.accent = vec4<f32>(srgb_to_linear(in.accent.rgb), in.accent.a);
    out.fill = vec4<f32>(srgb_to_linear(in.fill.rgb), in.fill.a);
    out.gauges = in.gauges;
    out.parts_a = in.parts_a;
    out.parts_b = in.parts_b;
    out.fade = in.fade * locals.offset.w;
    out.lift = in.position.y - in.base;
    out.badge = in.badge;
    return out;
}

@vertex
fn token_vertex(@builtin(vertex_index) index: u32, in: TokenIn) -> TokenOut {
    let corner = token_corner(index);
    // Room for rings, gauges and the hits bar around the silhouette.
    let half = max(in.size, 1.0) * 0.5 + 0.08;
    let world = locals.offset.xyz + in.position + vec3<f32>(corner.x * half, 0.0, corner.y * half);
    return token_out(in, corner, half, world);
}

// The shadow of a token with a prism: its silhouette on the ground, cast away from the light.
@vertex
fn shadow_vertex(@builtin(vertex_index) index: u32, in: TokenIn) -> TokenOut {
    let corner = token_corner(index);
    let height = in.position.y - in.base;
    let half = in.size * 0.5 + 0.12 + height * 0.25;
    let away = -globals.light.xz / max(globals.light.y, 0.3) * height * 0.55;
    var ground = in.position;
    ground.y = in.base + 0.004;
    let world = locals.offset.xyz + ground + vec3<f32>(corner.x * half + away.x, 0.0, corner.y * half + away.y);
    var out = token_out(in, corner, half, world);
    if ((in.kind >> 16u) & 255u) == NO_SHAPE {
        // Flat objects cast none: outside the clip volume.
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    return out;
}

// --- Shapes -----------------------------------------------------------------------------

fn wrap(x: f32, m: f32) -> f32 {
    return x - m * floor(x / m);
}

// A rounded regular polygon (see `mesh::Polygon`): `sides` 0 is a circle.
fn sd_ngon(p: vec2<f32>, sides: f32, angle: f32, apothem: f32, rounding: f32) -> f32 {
    if sides < 1.0 {
        return length(p) - apothem;
    }
    let sector = TAU / sides;
    let half = sector * 0.5;
    let a = atan2(p.x, -p.y);
    let local = wrap(a - angle + half, sector) - half;
    let len = length(p);
    let q = vec2<f32>(len * cos(local), abs(len * sin(local)));
    let inner = apothem - rounding;
    let x = q.x - inner;
    let y = q.y - inner * tan(half);
    return length(vec2<f32>(x, max(y, 0.0))) * sign(x) - rounding;
}

// The silhouette `shape` (`mesh::Shape` order) of footprint width `size`.
fn silhouette(shape: u32, p: vec2<f32>, size: f32) -> f32 {
    let q = p / max(size, 1e-3);
    var d: f32;
    switch shape {
        case 1u: { d = sd_ngon(q, 4.0, 0.0, 0.5, 0.1); }
        case 2u: { d = sd_ngon(q, 4.0, PI * 0.25, 0.353553, 0.05); }
        case 3u: { d = sd_ngon(q, 6.0, 0.0, 0.433013, 0.05); }
        case 4u: { d = sd_ngon(q, 8.0, 0.0, 0.5, 0.05); }
        case 5u: { d = sd_ngon(q, 3.0, PI, 0.25, 0.06); }
        case 6u: { d = sd_ngon(q, 5.0, PI, 0.404508, 0.05); }
        default: { d = length(q) - 0.5; }
    }
    return d * size;
}

fn sd_box(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn sd_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

// Direction `index` (0 north, clockwise; `scene::connect`) as a tile offset.
fn link_direction(index: u32) -> vec2<f32> {
    var directions = array<vec2<f32>, 8>(
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 0.0),
        vec2<f32>(-1.0, -1.0),
    );
    return directions[index];
}

// Roads: a disc and a capsule towards every linked neighbour. Links run past the shared
// edge (and past the token's square), so neighbours draw the same road where their squares
// overlap: no caps or seams between tiles.
fn sd_road(p: vec2<f32>, links: u32, width: f32) -> f32 {
    var d = length(p) - width;
    for (var i = 0u; i < 8u; i = i + 1u) {
        if ((links >> i) & 1u) != 0u {
            d = min(d, sd_segment(p, vec2<f32>(0.0), link_direction(i) * 0.75) - width);
        }
    }
    return d;
}

// Walls and ramparts: a block merged with linked orthogonal neighbours, the corner between
// three filled.
fn sd_block(p: vec2<f32>, links: u32, half: f32, radius: f32) -> f32 {
    var d = sd_box(p, vec2<f32>(half), radius);
    for (var i = 0u; i < 8u; i = i + 2u) {
        if ((links >> i) & 1u) != 0u {
            // An arm from the centre past the shared edge, as wide as the block (the
            // token's square is clipped to its tile, see `tile_clip`).
            let direction = link_direction(i);
            let extent = abs(direction) * 0.4 + abs(direction.yx) * half;
            d = min(d, sd_box(p - direction * 0.4, extent, 0.0));
        }
    }
    for (var i = 1u; i < 8u; i = i + 2u) {
        if ((links >> i) & 1u) != 0u {
            let direction = link_direction(i);
            d = min(d, sd_box(p - direction * 0.25, vec2<f32>(0.25), 0.0));
        }
    }
    return d;
}

// 1 inside the token's own tile, 0 outside: joined blocks are translucent (ramparts), so
// neighbours must not draw over each other.
fn tile_clip(p: vec2<f32>) -> f32 {
    return step(max(abs(p.x), abs(p.y)), 0.5);
}

// --- Colours ------------------------------------------------------------------------------

fn rgb8(r: f32, g: f32, b: f32) -> vec3<f32> {
    return srgb_to_linear(vec3<f32>(r, g, b) / 255.0);
}

// Premultiplied "over".
fn over(acc: vec4<f32>, rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return acc * (1.0 - a) + vec4<f32>(rgb * a, a);
}

fn cover(sd: f32, px: f32) -> f32 {
    return clamp(0.5 - sd / px, 0.0, 1.0);
}

// Coverage of the band `inner..outer` of a distance (a ring).
fn band(sd: f32, inner: f32, outer: f32, px: f32) -> f32 {
    return cover(sd - outer, px) * (1.0 - cover(sd - inner, px));
}

fn part_color(index: u32) -> vec3<f32> {
    var colors = array<vec3<f32>, 8>(
        vec3<f32>(168.0, 182.0, 200.0),
        vec3<f32>(242.0, 201.0, 76.0),
        vec3<f32>(138.0, 122.0, 104.0),
        vec3<f32>(232.0, 74.0, 74.0),
        vec3<f32>(90.0, 141.0, 224.0),
        vec3<f32>(95.0, 208.0, 122.0),
        vec3<f32>(168.0, 122.0, 230.0),
        vec3<f32>(230.0, 230.0, 230.0),
    );
    return srgb_to_linear(colors[index] / 255.0);
}

// A gauge byte (0: none, 1..=255: 0..=1).
fn gauge(value: u32) -> f32 {
    return (f32(value) - 1.0) / 254.0;
}

// Angle of `p` from north, clockwise, as a turn fraction `0..1`.
fn turn(p: vec2<f32>) -> f32 {
    return wrap(atan2(p.x, -p.y) / TAU, 1.0);
}

// --- Emblems ------------------------------------------------------------------------------

struct Emblem {
    color: vec4<f32>,
    // The silhouette's distance (for the bevel and the summary when zoomed out).
    sd: f32,
    // The summary colour when zoomed out.
    summary: vec3<f32>,
}

// A structure's body: dark edge, slate body, owner rim, inner panel.
fn body(sd: f32, px: f32, rim: vec3<f32>, panel: f32) -> vec4<f32> {
    var acc = vec4<f32>(0.0);
    acc = over(acc, rgb8(24.0, 26.0, 31.0), cover(sd, px));
    acc = over(acc, rim, cover(sd + 0.025, px));
    acc = over(acc, rgb8(58.0, 63.0, 73.0), cover(sd + 0.068, px));
    acc = over(acc, rgb8(40.0, 44.0, 51.0), cover(sd + panel, px));
    return acc;
}

// The stored resource as a glowing core of radius ∝ √fill.
fn core(acc: vec4<f32>, p: vec2<f32>, radius: f32, fill: vec4<f32>, px: f32) -> vec4<f32> {
    if fill.a <= 0.0 {
        return acc;
    }
    let amount = clamp((fill.a * 255.0 - 1.0) / 254.0, 0.0, 1.0);
    let r = radius * sqrt(amount);
    let d = length(p) - r;
    var out = over(acc, fill.rgb * 0.35, cover(d - 0.035, px) * step(0.001, amount));
    out = over(out, fill.rgb, cover(d, px));
    return out;
}

// The stored resource as a level filling the region `sd_panel < 0` from the south.
fn level(acc: vec4<f32>, p: vec2<f32>, sd_panel: f32, extent: f32, fill: vec4<f32>, px: f32) -> vec4<f32> {
    if fill.a <= 0.0 {
        return acc;
    }
    let amount = clamp((fill.a * 255.0 - 1.0) / 254.0, 0.0, 1.0);
    let top = extent - 2.0 * extent * amount;
    let inside = cover(sd_panel, px) * cover(top - p.y, px);
    return over(acc, fill.rgb * 0.85, inside);
}

// A progress arc from north, clockwise.
fn arc(acc: vec4<f32>, p: vec2<f32>, progress: f32, inner: f32, outer: f32, color: vec3<f32>, px: f32) -> vec4<f32> {
    let r = length(p);
    let ring = band(r, inner, outer, px);
    let lit = step(turn(p), progress);
    var out = over(acc, color * 0.25, ring);
    out = over(out, color, ring * lit);
    return out;
}

// `count` of 8 pips around a circle of `radius`, the first `lit` in `color`.
fn pips(acc: vec4<f32>, p: vec2<f32>, lit: u32, count: u32, radius: f32, size: f32, color: vec3<f32>, px: f32) -> vec4<f32> {
    let t = turn(p) * f32(count);
    let index = u32(floor(t + 0.5)) % count;
    let angle = f32(index) / f32(count) * TAU;
    let centre = vec2<f32>(sin(angle), -cos(angle)) * radius;
    let d = length(p - centre) - size;
    let on = select(rgb8(70.0, 75.0, 86.0), color, index < lit);
    return over(acc, on, cover(d, px));
}

// The heading of a token (`gauges.w`: 256ths of a turn clockwise from north) as a turn
// fraction.
fn heading_of(gauges: vec4<u32>) -> f32 {
    return f32(gauges.w) / 256.0;
}

// How far `p` is round from the back (0) to the front (1, the `heading`), on either side.
fn from_back(p: vec2<f32>, heading: f32) -> f32 {
    return abs(wrap(turn(p) - heading, 1.0) - 0.5) * 2.0;
}

// The creep's body-part ring: one segment per part, grouped by type and mirrored about the
// heading, front to back: attack, ranged attack, heal, claim, work, tough, carry, move.
fn parts_ring(acc: vec4<f32>, p: vec2<f32>, parts_a: vec4<u32>, parts_b: vec4<u32>, heading: f32, inner: f32, outer: f32, px: f32) -> vec4<f32> {
    var counts = array<u32, 8>(parts_a.x, parts_a.y, parts_a.z, parts_a.w, parts_b.x, parts_b.y, parts_b.z, parts_b.w);
    var total = 0u;
    for (var i = 0u; i < 8u; i = i + 1u) {
        total = total + counts[i];
    }
    if total == 0u {
        return acc;
    }
    let r = length(p);
    let ring = band(r, inner, outer, px);
    if ring <= 0.0 {
        return acc;
    }
    var order = array<u32, 8>(3u, 4u, 5u, 6u, 1u, 7u, 2u, 0u);
    // Each part takes a `1 / total` share of either side of the ring.
    let position = min((1.0 - from_back(p, heading)) * f32(total), f32(total) - 1e-3);
    var start = 0u;
    var color = part_color(0u);
    for (var i = 0u; i < 8u; i = i + 1u) {
        let kind = order[i];
        let count = counts[kind];
        if position >= f32(start) && position < f32(start + count) {
            color = part_color(kind);
        }
        start = start + count;
    }
    // Thin gaps between parts, when they are wide enough to show; none across the front and
    // the back, where a part meets its mirror image.
    let width = PI * (inner + outer) * 0.5 / f32(total);
    let nearest = round(position);
    let inside = nearest >= 1.0 && nearest <= f32(total) - 1.0;
    let gap = abs(position - nearest) * width;
    let gaps = select(1.0, smoothstep(0.0, px * 1.2, gap - px * 0.25), inside && width > px * 4.0);
    var out = over(acc, rgb8(18.0, 20.0, 24.0), ring);
    out = over(out, color, ring * gaps);
    return out;
}

// A creep's store as a ring between `inner` and `outer` that fills from the back towards
// the heading on both sides: dim where empty while the creep has `carry` parts.
fn store_ring(acc: vec4<f32>, p: vec2<f32>, heading: f32, fill: vec4<f32>, carry: u32, inner: f32, outer: f32, px: f32) -> vec4<f32> {
    if fill.a <= 0.0 && carry == 0u {
        return acc;
    }
    let ring = band(length(p), inner, outer, px);
    let amount = clamp((fill.a * 255.0 - 1.0) / 254.0, 0.0, 1.0) * step(0.001, fill.a);
    // The lit edge, antialiased: a pixel is about this share of half the ring.
    let edge = px / (PI * (inner + outer) * 0.5);
    let lit = clamp((amount - from_back(p, heading)) / edge + 0.5, 0.0, 1.0) * step(0.001, amount);
    var out = over(acc, rgb8(46.0, 50.0, 58.0), ring);
    out = over(out, fill.rgb, ring * lit);
    return out;
}

// The owner's badge (`TokenOut::badge`: its atlas cell plus one) on the disc of `radius`
// round the token's centre, premultiplied, its top turned to `heading` (a turn fraction
// clockwise from north). `dx` and `dy` are the screen derivatives of `p` (sampled with
// explicit gradients: emblems run in non-uniform control flow).
fn badge_disc(badge: u32, p: vec2<f32>, heading: f32, radius: f32, dx: vec2<f32>, dy: vec2<f32>, px: f32) -> vec4<f32> {
    let cell = badge - 1u;
    let block = cell % BADGE_CELLS_PER_LAYER;
    let corner = vec2<f32>(f32(block % BADGE_COLUMNS), f32(block / BADGE_COLUMNS));
    let scale = vec2<f32>(1.0 / f32(BADGE_COLUMNS), 1.0 / f32(BADGE_ROWS));
    // From the tile (y south) into the badge: turned back by the heading.
    let angle = heading * TAU;
    let c = cos(angle);
    let s = sin(angle);
    let back = mat2x2<f32>(vec2<f32>(c, -s), vec2<f32>(s, c));
    let q = back * p;
    let unit = clamp(q / (2.0 * radius) + vec2<f32>(0.5), vec2<f32>(0.02), vec2<f32>(0.98));
    let k = scale / (2.0 * radius);
    let color = textureSampleGrad(
        badge_atlas,
        badge_sampler,
        (corner + unit) * scale,
        i32(cell / BADGE_CELLS_PER_LAYER),
        (back * dx) * k,
        (back * dy) * k,
    );
    return color * cover(length(p) - radius, px);
}

// Premultiplied `top` over `acc`.
fn over_premultiplied(acc: vec4<f32>, top: vec4<f32>) -> vec4<f32> {
    return acc * (1.0 - top.a) + top;
}

fn hits_bar(acc: vec4<f32>, p: vec2<f32>, hits: u32, px: f32) -> vec4<f32> {
    if hits == 0u {
        return acc;
    }
    let left = gauge(hits);
    let centre = vec2<f32>(0.0, 0.43);
    let back = sd_box(p - centre, vec2<f32>(0.4, 0.045), 0.02);
    let x = -0.38 + 0.76 * left;
    let fill = max(sd_box(p - centre, vec2<f32>(0.38, 0.025), 0.01), p.x - x);
    let color = mix(rgb8(232.0, 74.0, 74.0), mix(rgb8(242.0, 201.0, 76.0), rgb8(95.0, 208.0, 122.0), smoothstep(0.5, 0.9, left)), smoothstep(0.1, 0.5, left));
    var out = over(acc, rgb8(14.0, 15.0, 18.0), cover(back, px) * 0.9);
    out = over(out, color, cover(fill, px));
    return out;
}

fn emblem(in: TokenOut, p: vec2<f32>, px: f32, dx: vec2<f32>, dy: vec2<f32>) -> Emblem {
    let glyph = in.kind & 255u;
    let links = (in.kind >> 8u) & 255u;
    let shape = (in.kind >> 16u) & 255u;
    let flags = in.kind >> 24u;
    let owned = in.accent.a > 0.5;
    let neutral = rgb8(150.0, 156.0, 168.0);
    let rim = select(neutral, in.accent.rgb, owned);
    let r = in.size * 0.5;
    var e: Emblem;
    e.summary = rim;
    var sd = 1.0;
    if shape != NO_SHAPE {
        sd = silhouette(shape, p, in.size);
    }
    var acc = vec4<f32>(0.0);
    switch glyph {
        // Creep: owner rim, body parts mirrored about the heading, the store ring and the
        // owner's badge (a dark core with the store's glow until the badge is there).
        case 0u: {
            let d = length(p);
            let heading = heading_of(in.gauges);
            acc = over(acc, rgb8(18.0, 20.0, 24.0), cover(sd, px));
            acc = over(acc, rim, band(d, r - 0.045, r - 0.01, px));
            acc = parts_ring(acc, p, in.parts_a, in.parts_b, heading, r * 0.64, r * 0.86, px);
            if in.badge > 0u {
                acc = store_ring(acc, p, heading, in.fill, in.parts_a.z, r * 0.5, r * 0.59, px);
                // The badge's top faces the front, as the creep turns.
                acc = over_premultiplied(acc, badge_disc(in.badge, p, heading, r * 0.46, dx, dy, px));
            } else {
                acc = over(acc, mix(rgb8(30.0, 33.0, 39.0), rim, 0.15), cover(d - r * 0.6, px));
                acc = core(acc, p, r * 0.48, in.fill, px);
            }
        }
        // Power creep: a pentagon with a crimson ring round the owner's badge.
        case 1u: {
            acc = body(sd, px, rim, 0.1);
            let ring = sd_ngon(p, 5.0, PI, r * 0.6, 0.03);
            acc = over(acc, rgb8(255.0, 61.0, 110.0), band(ring, -0.03, 0.0, px));
            if in.badge > 0u {
                // Upright: the pentagon does not turn.
                acc = over_premultiplied(acc, badge_disc(in.badge, p, 0.0, r * 0.44, dx, dy, px));
            } else {
                acc = core(acc, p, r * 0.36, in.fill, px);
            }
        }
        // Spawn.
        case 2u: {
            acc = body(sd, px, rim, 0.12);
            acc = over(acc, rim * 0.6, band(length(p), r * 0.58, r * 0.64, px));
            acc = core(acc, p, r * 0.5, in.fill, px);
            if (flags & SPAWNING) != 0u {
                let pulse = 0.75 + 0.25 * sin(globals.eye.w * 6.0);
                acc = arc(acc, p, gauge(in.gauges.y), r * 0.66, r * 0.8, rgb8(255.0, 240.0, 200.0) * pulse, px);
            }
        }
        // Extension.
        case 3u: {
            acc = body(sd, px, rim, 0.09);
            acc = core(acc, p, r * 0.5, in.fill, px);
        }
        // Tower: a turret whose barrel points at its last target.
        case 4u: {
            acc = body(sd, px, rim, 0.12);
            let angle = heading_of(in.gauges) * TAU;
            let aim = vec2<f32>(sin(angle), -cos(angle));
            let barrel = sd_segment(p, aim * r * 0.2, aim * r * 0.95) - r * 0.12;
            let turret = length(p) - r * 0.52;
            let gun = min(turret, barrel);
            acc = over(acc, rgb8(20.0, 22.0, 26.0), cover(gun, px));
            acc = over(acc, rgb8(70.0, 76.0, 88.0), cover(turret + 0.03, px));
            acc = over(acc, rgb8(118.0, 126.0, 140.0), cover(barrel + 0.025, px));
            acc = core(acc, p, r * 0.4, in.fill, px);
        }
        // Storage.
        case 5u: {
            acc = body(sd, px, rim, 0.12);
            let panel = sd_box(p, vec2<f32>(r - 0.13), 0.04);
            acc = level(acc, p, panel, r - 0.13, in.fill, px);
        }
        // Terminal.
        case 6u: {
            acc = body(sd, px, rim, 0.12);
            let panel = sd_ngon(p, 4.0, PI * 0.25, r * 0.707 - 0.12, 0.02);
            acc = level(acc, p, panel, r - 0.16, in.fill, px);
            acc = over(acc, rim * 0.8, band(sd_ngon(p, 4.0, PI * 0.25, r * 0.25, 0.01), -0.03, 0.0, px));
        }
        // Container.
        case 7u: {
            acc = body(sd, px, rgb8(150.0, 128.0, 92.0), 0.08);
            let panel = sd_box(p, vec2<f32>(r - 0.08), 0.02);
            acc = level(acc, p, panel, r - 0.08, in.fill, px);
            e.summary = rgb8(150.0, 128.0, 92.0);
        }
        // Link.
        case 8u: {
            acc = body(sd, px, rim, 0.08);
            acc = core(acc, p, r * 0.45, in.fill, px);
        }
        // Lab.
        case 9u: {
            acc = body(sd, px, rim, 0.1);
            acc = over(acc, rgb8(24.0, 26.0, 31.0), band(length(p), r * 0.62, r * 0.68, px));
            acc = core(acc, p, r * 0.55, in.fill, px);
        }
        // Factory.
        case 10u: {
            acc = body(sd, px, rim, 0.12);
            let panel = sd_ngon(p, 6.0, 0.0, r * 0.866 - 0.12, 0.03);
            acc = level(acc, p, panel, r * 0.866 - 0.12, in.fill, px);
            acc = pips(acc, p, in.gauges.z, 5u, r * 0.5, 0.04, rgb8(255.0, 220.0, 120.0), px * step(0.5, f32(in.gauges.z)) + px);
        }
        // Power spawn.
        case 11u: {
            acc = body(sd, px, rim, 0.12);
            acc = over(acc, rgb8(255.0, 61.0, 110.0), band(length(p), r * 0.5, r * 0.6, px));
            acc = core(acc, p, r * 0.42, in.fill, px);
        }
        // Nuker.
        case 12u: {
            acc = body(sd, px, rim, 0.1);
            acc = core(acc, p + vec2<f32>(0.0, -0.06), r * 0.3, in.fill, px);
        }
        // Observer.
        case 13u: {
            acc = body(sd, px, rim, 0.08);
            acc = over(acc, rgb8(140.0, 210.0, 255.0), band(length(p), r * 0.25, r * 0.42, px));
        }
        // Extractor: a toothed ring around its mineral.
        case 14u: {
            let ring = length(p);
            let teeth = abs(fract(turn(p) * 8.0) - 0.5);
            let d = max(abs(ring - 0.42) - 0.045, -(teeth - 0.18) * 0.6);
            sd = abs(ring - 0.42) - 0.06;
            acc = over(acc, rgb8(24.0, 26.0, 31.0), cover(abs(ring - 0.42) - 0.06, px) * 0.8);
            acc = over(acc, rim, cover(d, px));
        }
        // Controller.
        case 15u: {
            acc = body(sd, px, rim, 0.1);
            acc = pips(acc, p, in.gauges.z, 8u, r * 0.48, 0.045, rim, px);
            if in.gauges.y > 0u {
                acc = arc(acc, p, gauge(in.gauges.y), r * 0.62, r * 0.72, mix(rim, vec3<f32>(1.0), 0.35), px);
            }
            acc = over(acc, rgb8(24.0, 26.0, 31.0), cover(length(p) - r * 0.2, px));
            acc = over(acc, rim * 0.8, cover(length(p) - r * 0.12, px));
        }
        // Source.
        case 16u: {
            let gold = rgb8(255.0, 209.0, 58.0);
            acc = over(acc, rgb8(70.0, 56.0, 18.0), cover(sd, px));
            acc = over(acc, gold * 0.5, cover(sd + 0.03, px));
            let amount = select(0.0, gauge(u32(in.fill.a * 255.0 + 0.5)), in.fill.a > 0.0);
            let inner = sd_box(p, vec2<f32>((r - 0.08) * sqrt(amount)), 0.05);
            acc = over(acc, gold, cover(inner, px));
            e.summary = gold;
        }
        // Mineral.
        case 17u: {
            let gem = in.fill.rgb;
            acc = over(acc, gem * 0.25, cover(sd, px));
            acc = over(acc, gem * 0.7, cover(sd + 0.04, px));
            let facet = sd_ngon(p, 6.0, 0.0, r * 0.45, 0.02);
            acc = over(acc, gem, cover(facet, px));
            e.summary = gem;
        }
        // Deposit.
        case 18u: {
            let blue = rgb8(110.0, 158.0, 196.0);
            acc = over(acc, blue * 0.3, cover(sd, px));
            acc = over(acc, blue * 0.8, cover(sd + 0.05, px));
            e.summary = blue;
        }
        // Power bank.
        case 19u: {
            let red = rgb8(255.0, 61.0, 110.0);
            acc = over(acc, red * 0.25, cover(sd, px));
            acc = over(acc, red * 0.7, cover(sd + 0.05, px));
            acc = over(acc, red, cover(length(p) - r * 0.3, px));
            e.summary = red;
        }
        // Constructed wall.
        case 20u: {
            sd = sd_block(p, links, 0.44, 0.08);
            let clip = tile_clip(p);
            acc = over(acc, rgb8(40.0, 43.0, 49.0), cover(sd, px) * clip);
            acc = over(acc, rgb8(92.0, 98.0, 108.0), cover(sd + 0.04, px) * clip);
            e.summary = rgb8(92.0, 98.0, 108.0);
        }
        // Rampart: translucent over what it protects.
        case 21u: {
            sd = sd_block(p, links, 0.47, 0.14);
            let clip = tile_clip(p);
            acc = over(acc, rim, cover(sd, px) * 0.26 * clip);
            acc = over(acc, rim, band(sd, -0.06, 0.0, px) * 0.9 * clip);
            e.summary = rim;
        }
        // Road.
        case 22u: {
            sd = sd_road(p, links, 0.15);
            acc = over(acc, rgb8(70.0, 73.0, 79.0), cover(sd, px));
            acc = over(acc, rgb8(88.0, 92.0, 99.0), cover(sd + 0.05, px));
            e.summary = rgb8(84.0, 88.0, 95.0);
        }
        // Construction site.
        case 23u: {
            let ring = length(p) - r * 0.8;
            let dashes = step(0.5, fract(turn(p) * 12.0));
            acc = over(acc, rim, band(ring, -0.035, 0.0, px) * dashes);
            acc = arc(acc, p, gauge(in.gauges.y), r * 0.4, r * 0.62, rim, px);
            acc = over(acc, rim, cover(length(p) - r * 0.16, px));
            sd = ring;
        }
        // Dropped resource.
        case 24u: {
            sd = length(p) - r;
            acc = over(acc, in.fill.rgb * 0.3, cover(sd, px));
            acc = over(acc, in.fill.rgb, cover(sd + 0.03, px));
            acc = over(acc, mix(in.fill.rgb, vec3<f32>(1.0), 0.5), cover(length(p - vec2<f32>(-r * 0.3, -r * 0.3)) - r * 0.25, px) * 0.6);
            e.summary = in.fill.rgb;
        }
        // Tombstone.
        case 25u: {
            acc = over(acc, rgb8(30.0, 32.0, 36.0), cover(sd, px));
            acc = over(acc, rgb8(120.0, 124.0, 132.0), cover(sd + 0.03, px));
            let cross = min(sd_box(p, vec2<f32>(0.025, r * 0.6), 0.0), sd_box(p + vec2<f32>(0.0, r * 0.15), vec2<f32>(r * 0.45, 0.025), 0.0));
            acc = over(acc, rgb8(30.0, 32.0, 36.0), cover(cross, px));
            acc = core(acc, p, r * 0.25, in.fill, px);
            e.summary = rgb8(120.0, 124.0, 132.0);
        }
        // Ruin.
        case 26u: {
            let a = sd_box(p - vec2<f32>(-0.12, 0.06), vec2<f32>(0.12, 0.09), 0.02);
            let b = sd_box(p - vec2<f32>(0.13, -0.08), vec2<f32>(0.1, 0.08), 0.02);
            let c = sd_box(p - vec2<f32>(0.1, 0.16), vec2<f32>(0.07, 0.06), 0.02);
            sd = min(a, min(b, c));
            acc = over(acc, rgb8(30.0, 28.0, 25.0), cover(sd, px));
            acc = over(acc, rgb8(110.0, 96.0, 78.0), cover(sd + 0.025, px));
            e.summary = rgb8(110.0, 96.0, 78.0);
        }
        // Nuke landing.
        case 27u: {
            let pulse = 0.6 + 0.4 * sin(globals.eye.w * 4.0);
            let red = rgb8(255.0, 60.0, 50.0);
            let ring = abs(length(p) - 0.36) - 0.03;
            let cross = min(sd_box(p, vec2<f32>(0.46, 0.015), 0.0), sd_box(p, vec2<f32>(0.015, 0.46), 0.0));
            sd = min(ring, cross);
            acc = over(acc, red * pulse, cover(sd, px));
            e.summary = red;
        }
        // Portal.
        case 28u: {
            let swirl = fract(turn(p) * 3.0 - length(p) * 2.5 + globals.eye.w * 0.4);
            let cyan = rgb8(90.0, 209.0, 255.0);
            sd = length(p) - 0.45;
            acc = over(acc, cyan * 0.25, cover(sd, px));
            acc = over(acc, cyan, cover(sd, px) * smoothstep(0.45, 0.6, swirl) * (1.0 - smoothstep(0.6, 0.9, swirl)));
            e.summary = cyan;
        }
        // Keeper lair.
        case 29u: {
            acc = body(sd, px, rim, 0.12);
            acc = over(acc, rgb8(14.0, 15.0, 18.0), cover(length(p) - r * 0.4, px));
        }
        // Invader core.
        case 30u: {
            acc = body(sd, px, rim, 0.12);
            acc = pips(acc, p, in.gauges.z, 5u, r * 0.45, 0.045, rim, px);
            acc = over(acc, rim, cover(length(p) - r * 0.18, px));
        }
        default: {
            acc = body(sd, px, rim, 0.08);
        }
    }
    acc = hits_bar(acc, p, in.gauges.x, px);
    e.color = acc;
    e.sd = sd;
    return e;
}

@fragment
fn token_fragment(in: TokenOut) -> @location(0) vec4<f32> {
    let p = in.local;
    // Screen derivatives in uniform control flow: badges sample with them.
    let dx = dpdx(p);
    let dy = dpdy(p);
    // Tiles per pixel.
    let px = max(length(abs(dx) + abs(dy)) * 0.75, 1e-4);
    let e = emblem(in, p, px, dx, dy);
    var color = e.color;
    // Zoomed out (tiles of a few pixels): the silhouette in one summary colour.
    let simple = smoothstep(0.045, 0.12, px);
    if simple > 0.0 {
        let summary = vec4<f32>(e.summary, 1.0) * cover(e.sd, px) * color.a;
        color = mix(color, summary, simple);
    }
    // Light from above, a bevel along the silhouette's rim lit from the light's side.
    let shape = (in.kind >> 16u) & 255u;
    var light = 0.6 + 0.4 * max(globals.light.y, 0.0);
    if shape != NO_SHAPE && simple < 1.0 {
        let e2 = vec2<f32>(0.01, 0.0);
        let gradient = vec2<f32>(
            silhouette(shape, p + e2.xy, in.size) - silhouette(shape, p - e2.xy, in.size),
            silhouette(shape, p + e2.yx, in.size) - silhouette(shape, p - e2.yx, in.size),
        );
        let outward = normalize(gradient + vec2<f32>(1e-6));
        let rim = 1.0 - smoothstep(0.0, 0.06, -e.sd);
        let facing = dot(outward, normalize(globals.light.xz + vec2<f32>(1e-6)));
        light = light * (1.0 + rim * facing * 0.35 * (1.0 - simple));
    }
    var fade = in.fade;
    if ((in.kind >> 24u) & GHOST) != 0u {
        // Fading out: towards grey.
        color = vec4<f32>(mix(color.rgb, vec3<f32>(dot(color.rgb, vec3<f32>(0.3, 0.5, 0.2))) * color.a, 0.6), color.a);
    }
    return vec4<f32>(color.rgb * light, color.a) * fade;
}

@fragment
fn shadow_fragment(in: TokenOut) -> @location(0) vec4<f32> {
    let shape = (in.kind >> 16u) & 255u;
    let sd = silhouette(shape, in.local, in.size);
    let soft = 0.06 + in.lift * 0.25;
    let alpha = 0.42 * (1.0 - smoothstep(-soft, soft, sd)) * in.fade;
    return vec4<f32>(0.0, 0.0, 0.0, alpha);
}

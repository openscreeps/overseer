// Flat world layers (appended to common.wgsl), drawn on the ground in world tiles: the rooms
// (terrain, status and owner, borders) and minimap points, each an instanced quad built
// from the vertex index.

// Every room's terrain, packed 2 bits per tile: slot `s` is the 25×25 block at column
// `s % 1640 % 40`, row `s % 1640 / 40` of layer `s / 1640` (see `world/terrain.rs`).
@group(1) @binding(0) var terrain: texture_2d_array<u32>;
// The overview of the bounds: shares of walls (r), swamps (g) and plains (b) per texel.
@group(1) @binding(1) var overview: texture_2d<f32>;
@group(1) @binding(2) var overview_sampler: sampler;

// Terrain states of a room instance besides a slot.
const PENDING: u32 = 0xffffffffu;
const ABSENT: u32 = 0xfffffffeu;
const SLOT_SIDE: u32 = 25u;
const SLOT_COLUMNS: u32 = 40u;
const SLOTS_PER_LAYER: u32 = 1640u;

// Corner `index` (0..6) of a unit quad, as two triangles.
fn quad_corner(index: u32) -> vec2<f32> {
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

fn ground(position: vec2<f32>) -> vec4<f32> {
    return globals.view_projection * vec4<f32>(position.x, 0.0, position.y, 1.0);
}

// Darkening of a room border (about one pixel on each side) at room-local tile `local`,
// `width` tiles per pixel.
fn border(local: vec2<f32>, width: vec2<f32>) -> f32 {
    let edge = min(local, vec2<f32>(50.0) - local) / max(width, vec2<f32>(1e-4));
    return (1.0 - clamp(min(edge.x, edge.y), 0.0, 1.0)) * globals.params.z;
}

struct RoomOut {
    @builtin(position) position: vec4<f32>,
    @location(0) overlay: vec4<f32>,
    @location(1) tint: vec4<f32>,
    @location(2) local: vec2<f32>,
    @location(3) overview_uv: vec2<f32>,
    @location(4) @interpolate(flat) slot: u32,
}

@vertex
fn room_vertex(
    @builtin(vertex_index) index: u32,
    @location(0) origin: vec2<f32>,
    @location(1) slot: u32,
    @location(2) overlay: vec4<f32>,
    @location(3) tint: vec4<f32>,
) -> RoomOut {
    var out: RoomOut;
    out.local = quad_corner(index) * 50.0;
    out.overview_uv = (origin + out.local - globals.overview.xy) / globals.overview.zw;
    out.slot = slot;
    out.overlay = vec4<f32>(srgb_to_linear(overlay.rgb), overlay.a);
    out.tint = vec4<f32>(srgb_to_linear(tint.rgb), tint.a);
    if slot == ABSENT {
        // Outside the world: outside the clip volume.
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    } else {
        out.position = ground(origin + out.local);
    }
    return out;
}

// The code (0 plain, 1 wall, 2 swamp) of room-local tile `tile` in `slot`.
fn tile_code(slot: u32, tile: vec2<f32>) -> u32 {
    let t = vec2<u32>(clamp(tile, vec2<f32>(0.0), vec2<f32>(49.0)));
    let index = t.y * 50u + t.x;
    let byte = index >> 2u;
    let block = slot % SLOTS_PER_LAYER;
    let layer = slot / SLOTS_PER_LAYER;
    let texel = vec2<u32>(
        (block % SLOT_COLUMNS) * SLOT_SIDE + byte % SLOT_SIDE,
        (block / SLOT_COLUMNS) * SLOT_SIDE + byte / SLOT_SIDE,
    );
    let value = textureLoad(terrain, vec2<i32>(texel), i32(layer), 0).r;
    return (value >> ((index & 3u) * 2u)) & 3u;
}

fn tile_color(code: u32) -> vec3<f32> {
    if code == 1u {
        return globals.wall.rgb;
    }
    if code == 2u {
        return globals.swamp.rgb;
    }
    return globals.plain.rgb;
}

// Signed distance (tiles) to the region of a cell whose corners (NW, NE, SW, SE as 0 or
// 1) are of a kind, at cell coordinate `q`: `mesh/contour.rs` (`cell_sd`) has the rules.
fn cell_sd(c: vec4<f32>, q: vec2<f32>) -> f32 {
    let count = c.x + c.y + c.z + c.w;
    if count < 0.5 {
        return 1.0;
    }
    if count > 3.5 {
        return -1.0;
    }
    if count < 1.5 {
        let corner = vec2<f32>(c.y + c.w, c.z + c.w);
        return length(q - corner) - 0.5;
    }
    if count > 2.5 {
        let m = vec4<f32>(1.0) - c;
        let corner = vec2<f32>(m.y + m.w, m.z + m.w);
        return 0.5 - length(q - corner);
    }
    if c.x == c.w {
        if c.x > 0.5 {
            return min(length(q), length(q - vec2<f32>(1.0))) - 0.5;
        }
        return min(length(q - vec2<f32>(1.0, 0.0)), length(q - vec2<f32>(0.0, 1.0))) - 0.5;
    }
    if c.x == c.y {
        return select(0.5 - q.y, q.y - 0.5, c.x > 0.5);
    }
    return select(0.5 - q.x, q.x - 0.5, c.x > 0.5);
}

// Terrain at room-local `local` with smoothed outlines (`mesh/contour.rs`), `px` tiles per
// pixel: walls rounded, swamps blended in, a soft shadow on the ground around walls.
fn smooth_terrain(slot: u32, local: vec2<f32>, px: f32) -> vec3<f32> {
    let cell = floor(local - vec2<f32>(0.5));
    let q = local - vec2<f32>(0.5) - cell;
    let codes = vec4<u32>(
        tile_code(slot, cell),
        tile_code(slot, cell + vec2<f32>(1.0, 0.0)),
        tile_code(slot, cell + vec2<f32>(0.0, 1.0)),
        tile_code(slot, cell + vec2<f32>(1.0, 1.0)),
    );
    let walls = select(vec4<f32>(0.0), vec4<f32>(1.0), codes == vec4<u32>(1u));
    // Swamps run under walls: no plain seams where they meet.
    let swamps = select(vec4<f32>(0.0), vec4<f32>(1.0), codes != vec4<u32>(0u));
    let wall = cell_sd(walls, q);
    let swamp = cell_sd(swamps, q);
    let aa = max(px, 1e-4);
    var color = mix(globals.plain.rgb, globals.swamp.rgb, clamp(0.5 - swamp / aa, 0.0, 1.0));
    color = color * (1.0 - 0.4 * (1.0 - smoothstep(0.0, 0.5, wall)));
    return mix(color, globals.wall.rgb, clamp(0.5 - wall / aa, 0.0, 1.0));
}

@fragment
fn room_fragment(in: RoomOut) -> @location(0) vec4<f32> {
    // Tiles per pixel (derivatives and sampling in uniform control flow, before branches).
    let footprint = fwidth(in.local);
    // Lit like the flat tops of the detail terrain.
    let light = 0.6 + 0.4 * max(globals.light.y, 0.0);
    // The overview: area-averaged shares, filtered across rooms and mip levels; what no
    // terrain covers is the placeholder.
    let shares = textureSample(overview, overview_sampler, in.overview_uv).rgb;
    let missing = max(1.0 - shares.r - shares.g - shares.b, 0.0);
    var color = (globals.wall.rgb * shares.r + globals.swamp.rgb * shares.g
        + globals.plain.rgb * shares.b) * light + globals.pending.rgb * missing;
    let near = globals.params.x;
    if near > 0.0 {
        var tiles = globals.pending.rgb;
        if in.slot != PENDING {
            // Tiles of a few pixels and more: smoothed outlines; smaller ones: four taps a
            // quarter pixel apart (rotated grid) average what a pixel covers.
            let px = max(footprint.x, footprint.y);
            let sharp = 1.0 - smoothstep(0.2, 0.45, px);
            var averaged = vec3<f32>(0.0);
            if sharp < 1.0 {
                let a = tile_color(tile_code(in.slot, in.local + vec2<f32>(-0.125, -0.375) * footprint));
                let b = tile_color(tile_code(in.slot, in.local + vec2<f32>(0.375, -0.125) * footprint));
                let c = tile_color(tile_code(in.slot, in.local + vec2<f32>(0.125, 0.375) * footprint));
                let d = tile_color(tile_code(in.slot, in.local + vec2<f32>(-0.375, 0.125) * footprint));
                averaged = (a + b + c + d) * 0.25;
            }
            var smoothed = vec3<f32>(0.0);
            if sharp > 0.0 {
                smoothed = smooth_terrain(in.slot, in.local, px);
            }
            tiles = mix(averaged, smoothed, sharp) * light;
        }
        color = mix(color, tiles, near);
    }
    // Status and owner: a strong overlay zoomed out, a light tint over full resolution.
    let over = mix(in.overlay, in.tint, near);
    color = mix(color, over.rgb, over.a);
    color = color * (1.0 - 0.55 * border(in.local, footprint));
    return vec4<f32>(color, 1.0);
}

struct PointOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn point_vertex(
    @builtin(vertex_index) index: u32,
    @location(0) position: vec2<f32>,
    @location(1) size: f32,
    @location(2) color: vec4<f32>,
) -> PointOut {
    var out: PointOut;
    let corner = (quad_corner(index) - vec2<f32>(0.5)) * size;
    out.position = ground(position + vec2<f32>(0.5) + corner);
    out.color = vec4<f32>(srgb_to_linear(color.rgb), color.a * globals.params.y);
    return out;
}

@fragment
fn point_fragment(in: PointOut) -> @location(0) vec4<f32> {
    return in.color;
}

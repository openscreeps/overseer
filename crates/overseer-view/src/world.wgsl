// Flat world layers (appended to common.wgsl), drawn on the ground in world tiles: overview
// cells, atlas terrain and minimap points, each an instanced quad built from the vertex
// index.

@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

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

// Darkening of a room border (about one pixel on each side) at room-local tile `local`.
fn border(local: vec2<f32>) -> f32 {
    let width = max(fwidth(local), vec2<f32>(1e-4));
    let edge = min(local, vec2<f32>(50.0) - local) / width;
    return (1.0 - clamp(min(edge.x, edge.y), 0.0, 1.0)) * globals.params.z;
}

struct CellOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
}

@vertex
fn cell_vertex(
    @builtin(vertex_index) index: u32,
    @location(0) origin: vec2<f32>,
    @location(1) color: vec4<f32>,
) -> CellOut {
    var out: CellOut;
    out.local = quad_corner(index) * 50.0;
    out.position = ground(origin + out.local);
    out.color = vec4<f32>(srgb_to_linear(color.rgb), 1.0);
    return out;
}

@fragment
fn cell_fragment(in: CellOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.rgb * (1.0 - 0.55 * border(in.local)), 1.0);
}

struct TerrainOut {
    @builtin(position) position: vec4<f32>,
    @location(0) tint: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) cell: vec2<f32>,
}

@vertex
fn atlas_vertex(
    @builtin(vertex_index) index: u32,
    @location(0) origin: vec2<f32>,
    @location(1) cell: vec2<f32>,
    @location(2) tint: vec4<f32>,
) -> TerrainOut {
    var out: TerrainOut;
    out.local = quad_corner(index) * 50.0;
    out.cell = cell;
    out.tint = vec4<f32>(srgb_to_linear(tint.rgb), tint.a);
    if cell.x < 0.0 {
        // An empty slot: outside the clip volume.
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    } else {
        out.position = ground(origin + out.local);
    }
    return out;
}

@fragment
fn atlas_fragment(in: TerrainOut) -> @location(0) vec4<f32> {
    // Inset by one texel so that filtering never reads the neighbouring slot.
    let texel = clamp(in.local, vec2<f32>(1.0), vec2<f32>(49.0));
    let coverage = textureSample(atlas, atlas_sampler, (in.cell * 50.0 + texel) * globals.params.w).rg;
    let plain = max(1.0 - coverage.r - coverage.g, 0.0);
    var color = globals.plain.rgb * plain + globals.wall.rgb * coverage.r + globals.swamp.rgb * coverage.g;
    // Lit like the flat tops of the detail terrain.
    color = color * (0.6 + 0.4 * max(globals.light.y, 0.0));
    color = mix(color, in.tint.rgb, in.tint.a);
    color = color * (1.0 - 0.55 * border(in.local));
    return vec4<f32>(color, globals.params.x);
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

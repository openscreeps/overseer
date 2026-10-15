// Detail scenes (appended to common.wgsl): raised walls and object prisms in room-local
// tiles, moved to their room and faded by the per-room uniforms, and minimap points (world
// tiles) faded out under their room's detail scene. `objects.wgsl`, `effects.wgsl` and
// `visuals.wgsl` follow in the same module.

struct Locals {
    // xyz: world offset of the room; w: opacity.
    offset: vec4<f32>,
    // rgb: linear tint of the terrain; a: its strength (`accent` in common.wgsl).
    tint: vec4<f32>,
}

@group(1) @binding(0) var<uniform> locals: Locals;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn terrain_vertex(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
) -> Varyings {
    var out: Varyings;
    out.position = globals.view_projection * vec4<f32>(position + locals.offset.xyz, 1.0);
    // Wall sides darken towards the ground.
    let occlusion = select(1.0, 0.55 + 0.45 * clamp(position.y / 0.6, 0.0, 1.0), abs(normal.y) < 0.5);
    let shaded = lit(normal, color);
    out.color = vec4<f32>(accent(shaded.rgb * occlusion, locals.tint), locals.offset.w);
    return out;
}

@vertex
fn object_vertex(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) offset: vec3<f32>,
    @location(3) scale: vec3<f32>,
    @location(4) color: vec4<f32>,
) -> Varyings {
    var out: Varyings;
    let world = locals.offset.xyz + offset + position * scale;
    out.position = globals.view_projection * vec4<f32>(world, 1.0);
    // Normals of a scaled shape: inverse scale (scales are never zero).
    let shaded = lit(normal / scale, color);
    // Prism sides darken towards the ground.
    let occlusion = select(1.0, 0.6 + 0.4 * position.y, abs(normal.y) < 0.5);
    out.color = vec4<f32>(shaded.rgb * occlusion, shaded.a * locals.offset.w);
    return out;
}

// Corner `index` (0..6) of a centred unit quad, as two triangles.
fn point_corner(index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5),
        vec2<f32>(0.5, -0.5),
        vec2<f32>(0.5, 0.5),
        vec2<f32>(-0.5, -0.5),
        vec2<f32>(0.5, 0.5),
        vec2<f32>(-0.5, 0.5),
    );
    return corners[index];
}

// A minimap point: a square over a tile (world tiles) on the ground. `locals` are those of
// the room's detail scene (`offset.w` its opacity) or transparent: the scene, which draws
// every object at its true size, replaces the coarse minimap.
@vertex
fn point_vertex(
    @builtin(vertex_index) index: u32,
    @location(0) position: vec2<f32>,
    @location(1) size: f32,
    @location(2) color: vec4<f32>,
) -> Varyings {
    var out: Varyings;
    let corner = position + vec2<f32>(0.5) + point_corner(index) * size;
    out.position = globals.view_projection * vec4<f32>(corner.x, 0.0, corner.y, 1.0);
    let opacity = globals.params.y * (1.0 - locals.offset.w);
    out.color = vec4<f32>(srgb_to_linear(color.rgb), color.a * opacity);
    return out;
}

@fragment
fn fragment(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}

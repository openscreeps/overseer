// Detail scenes (appended to common.wgsl): terrain meshes and object shapes in room-local
// tiles, moved to their room and faded by the per-room uniforms.

struct Locals {
    // xyz: world offset of the room; w: opacity.
    offset: vec4<f32>,
    // rgb: linear tint of the terrain; a: its strength.
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
    let shaded = lit(normal, color);
    out.color = vec4<f32>(mix(shaded.rgb, locals.tint.rgb, locals.tint.a), locals.offset.w);
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
    out.color = vec4<f32>(shaded.rgb, shaded.a * locals.offset.w);
    return out;
}

@fragment
fn fragment(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}

// Terrain and object shapes, lit by one directional light (computed per vertex: every face is
// flat), drawn in linear colour into an sRGB target.

struct Globals {
    view_projection: mat4x4<f32>,
    // xyz: unit direction towards the light.
    light: vec4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

fn lit(normal: vec3<f32>, color: vec4<f32>) -> vec4<f32> {
    let light = 0.6 + 0.4 * max(dot(normalize(normal), globals.light.xyz), 0.0);
    return vec4<f32>(color.rgb * light, color.a);
}

@vertex
fn terrain_vertex(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
) -> Varyings {
    var out: Varyings;
    out.position = globals.view_projection * vec4<f32>(position, 1.0);
    out.color = lit(normal, color);
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
    out.position = globals.view_projection * vec4<f32>(offset + position * scale, 1.0);
    // Normals of a scaled shape: inverse scale (scales are never zero).
    out.color = lit(normal / scale, color);
    return out;
}

@fragment
fn fragment(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}

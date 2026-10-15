// Shared by both shader modules: the per-frame uniforms and colour helpers. Colours are
// linear, drawn into an sRGB target.

struct Globals {
    view_projection: mat4x4<f32>,
    // xyz: unit direction towards the light.
    light: vec4<f32>,
    // x: full-resolution terrain opacity over the overview, y: minimap opacity, z: room
    // border opacity, w: 0.
    params: vec4<f32>,
    // Terrain colours (linear) of plains, swamps and wall tops.
    plain: vec4<f32>,
    swamp: vec4<f32>,
    wall: vec4<f32>,
    // Colour (linear) of rooms whose terrain has not arrived.
    pending: vec4<f32>,
    // The world rectangle the overview texture covers: x, z of its corner, then its width
    // and depth (tiles).
    overview: vec4<f32>,
    // xyz: the camera's eye (world); w: animation time in seconds (wraps every hour).
    eye: vec4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

// One directional light, computed per vertex: every face is flat.
fn lit(normal: vec3<f32>, color: vec4<f32>) -> vec4<f32> {
    let light = 0.6 + 0.4 * max(dot(normalize(normal), globals.light.xyz), 0.0);
    return vec4<f32>(color.rgb * light, color.a);
}

fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    let low = color / 12.92;
    let high = pow((color + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, color <= vec3<f32>(0.04045));
}

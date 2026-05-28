struct Uniforms {
    proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.proj * vec4<f32>(in.pos, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_fill(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}

@fragment
fn fs_edge(in: VsOut) -> @location(0) vec4<f32> {
    // Brighten layer colour so the 1-pixel outline reads against the
    // fill underneath. clamp to keep dark-coloured layers visible.
    let bright = clamp(in.color * 1.5 + vec3<f32>(0.18), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(bright, 1.0);
}

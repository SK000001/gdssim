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

// Highlight is drawn as several offset passes (a dark halo under a bright
// core) so it reads against any layer colour and at any zoom. Each pass
// supplies its own colour and a projection with the pixel offset already
// baked into the translation, so the outline thickness is constant in
// screen space.
struct HiUniforms {
    proj: mat4x4<f32>,
    color: vec4<f32>,
    // x = time (s), y = stripe scale (world units per stripe).
    params: vec4<f32>,
};

@group(0) @binding(1) var<uniform> hi: HiUniforms;

@vertex
fn vs_highlight(in: VsIn) -> @builtin(position) vec4<f32> {
    return hi.proj * vec4<f32>(in.pos, 0.0, 1.0);
}

@fragment
fn fs_highlight() -> @location(0) vec4<f32> {
    return hi.color;
}

// Animated "signal flow" fill (H6): diagonal stripes scrolling in world
// space over time, so an active (driven-high) net visibly flows.
struct FlowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec2<f32>,
};

@vertex
fn vs_flow(in: VsIn) -> FlowOut {
    var out: FlowOut;
    out.clip = hi.proj * vec4<f32>(in.pos, 0.0, 1.0);
    out.world = in.pos;
    return out;
}

@fragment
fn fs_flow(in: FlowOut) -> @location(0) vec4<f32> {
    let t = hi.params.x;
    let scale = max(hi.params.y, 1.0);
    // Phase advances along the x+y diagonal and scrolls with time.
    let phase = (in.world.x + in.world.y) / scale - t;
    let s = 0.5 + 0.5 * sin(phase * 6.2831853);
    let a = hi.color.a * (0.5 + 0.5 * s);
    return vec4<f32>(hi.color.rgb, a);
}

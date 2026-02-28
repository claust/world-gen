struct FrameUniform {
    view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    time: vec4<f32>,
};

struct MaterialUniform {
    light_direction: vec4<f32>,
    ambient: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniform;
@group(1) @binding(0) var<uniform> material: MaterialUniform;

struct VertexInput {
    // Per-vertex (slot 0)
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) vert_color: vec3<f32>,

    // Per-instance (slot 1)
    @location(3) inst_position: vec3<f32>,
    @location(4) inst_rotation_y: f32,
    @location(5) inst_scale: vec3<f32>,
    @location(6) inst_color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) albedo: vec3<f32>,
    @location(2) world_position: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let c = cos(input.inst_rotation_y);
    let s = sin(input.inst_rotation_y);

    // Scale, then rotate around Y, then translate
    let scaled = input.position * input.inst_scale;
    let rotated = vec3<f32>(
        scaled.x * c - scaled.z * s,
        scaled.y,
        scaled.x * s + scaled.z * c,
    );
    let world_pos = rotated + input.inst_position;

    // Rotate normal (approximate for non-uniform scale)
    let rot_normal = vec3<f32>(
        input.normal.x * c - input.normal.z * s,
        input.normal.y,
        input.normal.x * s + input.normal.z * c,
    );

    var out: VertexOutput;
    out.clip_position = frame.view_proj * vec4<f32>(world_pos, 1.0);
    out.world_normal = rot_normal;
    out.albedo = input.vert_color * input.inst_color.rgb;
    out.world_position = world_pos;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(input.world_normal);
    let l = normalize(material.light_direction.xyz);
    let direct = max(dot(n, l), 0.0);
    let shade = material.ambient.x + direct * 0.82;
    let color = input.albedo * shade;

    let dist = distance(input.world_position, frame.camera_position.xyz);
    let fog_start = material.fog_params.x;
    let fog_end = material.fog_params.y;
    let fog_factor = clamp((dist - fog_start) / (fog_end - fog_start), 0.0, 1.0);
    let final_color = mix(color, material.fog_color.rgb, fog_factor);

    return vec4<f32>(final_color, 1.0);
}

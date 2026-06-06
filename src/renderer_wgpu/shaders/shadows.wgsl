// Depth-only shadow pass: renders scene geometry from the sun's point of view
// into the single-cascade shadow map. Only position (and the per-instance model
// matrix for instanced geometry) is needed.

struct FrameUniform {
    view_proj: mat4x4<f32>,
    inv_view_proj_no_translation: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    time: vec4<f32>,
    shadow_params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniform;

// Terrain / water-style geometry: position at location 0. Normal and color
// (locations 1/2) are present in the bound vertex buffer but unused here.
@vertex
fn vs_terrain(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return frame.light_view_proj * vec4<f32>(position, 1.0);
}

// Instanced geometry: per-vertex position (slot 0) plus the per-instance
// position / rotation / scale (slots 3..5), matching instanced.wgsl's layout.
@vertex
fn vs_instanced(
    @location(0) position: vec3<f32>,
    @location(3) inst_position: vec3<f32>,
    @location(4) inst_rotation_y: f32,
    @location(5) inst_scale: vec3<f32>,
) -> @builtin(position) vec4<f32> {
    let c = cos(inst_rotation_y);
    let s = sin(inst_rotation_y);
    let scaled = position * inst_scale;
    let rotated = vec3<f32>(
        scaled.x * c - scaled.z * s,
        scaled.y,
        scaled.x * s + scaled.z * c,
    );
    let world_pos = rotated + inst_position;
    return frame.light_view_proj * vec4<f32>(world_pos, 1.0);
}

// Depth-only; empty fragment stage to satisfy pipeline creation.
@fragment
fn fs_main() {}

struct ChunkParams {
    origin_x: f32,
    origin_z: f32,
    cell_size: f32,
    side: u32,
};

@group(0) @binding(0) var<uniform> params: ChunkParams;
@group(0) @binding(1) var<storage, read> heights: array<f32>;
@group(0) @binding(2) var<storage, read> moisture: array<f32>;
@group(0) @binding(3) var<storage, read_write> output: array<f32>;

fn biome_color(height: f32, moist: f32) -> vec3<f32> {
    var base: vec3<f32>;
    if (height > 165.0) {
        base = vec3<f32>(0.90, 0.92, 0.95); // Snow
    } else if (height > 120.0) {
        base = vec3<f32>(0.46, 0.48, 0.50); // Rock
    } else if (moist < 0.3) {
        base = vec3<f32>(0.70, 0.60, 0.36); // Desert
    } else if (moist > 0.62) {
        base = vec3<f32>(0.21, 0.43, 0.23); // Forest
    } else {
        base = vec3<f32>(0.34, 0.52, 0.24); // Grassland
    }

    let tint = clamp((height + 40.0) / 260.0, 0.0, 1.0);
    return mix(base, vec3<f32>(0.75, 0.75, 0.75), tint * 0.08);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x;
    let z = id.y;
    let side = params.side;

    if (x >= side || z >= side) {
        return;
    }

    let idx = z * side + x;
    let h = heights[idx];
    let m = moisture[idx];

    let world_x = params.origin_x + f32(x) * params.cell_size;
    let world_z = params.origin_z + f32(z) * params.cell_size;

    // Compute normal from neighboring heights
    let x0 = select(x - 1u, 0u, x == 0u);
    let x1 = min(x + 1u, side - 1u);
    let z0 = select(z - 1u, 0u, z == 0u);
    let z1 = min(z + 1u, side - 1u);

    let h_l = heights[z * side + x0];
    let h_r = heights[z * side + x1];
    let h_d = heights[z0 * side + x];
    let h_u = heights[z1 * side + x];

    let normal = normalize(vec3<f32>(h_l - h_r, params.cell_size * 2.0, h_d - h_u));

    let color = biome_color(h, m);

    // Write 9 floats per vertex (position, normal, color)
    let base = idx * 9u;
    output[base + 0u] = world_x;
    output[base + 1u] = h;
    output[base + 2u] = world_z;
    output[base + 3u] = normal.x;
    output[base + 4u] = normal.y;
    output[base + 5u] = normal.z;
    output[base + 6u] = color.x;
    output[base + 7u] = color.y;
    output[base + 8u] = color.z;
}

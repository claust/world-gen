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

// Biome IDs: 0=grass, 1=desert, 2=forest, 3=rock, 4=snow
// Returns vec3(biome_a, biome_b, blend_factor) with canonical ordering (biome_a <= biome_b)
fn biome_blend(height: f32, moist: f32) -> vec3<f32> {
    // Determine primary biome
    var primary: f32;
    var secondary: f32;
    var blend: f32 = 0.0;

    if (height > 165.0) {
        primary = 4.0; // Snow
        // Blend with rock near boundary
        let edge = smoothstep(155.0, 175.0, height);
        secondary = 3.0; // Rock
        blend = 1.0 - edge;
    } else if (height > 120.0) {
        primary = 3.0; // Rock
        // Blend with snow above, with lowland biome below
        let snow_edge = smoothstep(155.0, 175.0, height);
        let low_edge = smoothstep(110.0, 130.0, height);
        if (snow_edge > 0.01) {
            secondary = 4.0; // Snow
            blend = snow_edge;
        } else if (low_edge < 0.99) {
            // Blend with the lowland biome below
            if (moist < 0.3) {
                secondary = 1.0; // Desert
            } else if (moist > 0.62) {
                secondary = 2.0; // Forest
            } else {
                secondary = 0.0; // Grass
            }
            blend = 1.0 - low_edge;
        } else {
            secondary = 3.0;
            blend = 0.0;
        }
    } else if (moist < 0.3) {
        primary = 1.0; // Desert
        // Blend with grass near moisture boundary
        let edge = smoothstep(0.22, 0.38, moist);
        secondary = 0.0; // Grass
        blend = edge;
        // Blend with rock near height boundary
        let rock_edge = smoothstep(110.0, 130.0, height);
        if (rock_edge > blend) {
            secondary = 3.0;
            blend = rock_edge;
        }
    } else if (moist > 0.62) {
        primary = 2.0; // Forest
        // Blend with grass near moisture boundary
        let edge = smoothstep(0.54, 0.70, moist);
        secondary = 0.0; // Grass
        blend = 1.0 - edge;
        // Blend with rock near height boundary
        let rock_edge = smoothstep(110.0, 130.0, height);
        if (rock_edge > blend) {
            secondary = 3.0;
            blend = rock_edge;
        }
    } else {
        primary = 0.0; // Grass
        // Blend toward desert or forest near moisture boundaries
        let desert_edge = smoothstep(0.22, 0.38, moist);
        let forest_edge = smoothstep(0.54, 0.70, moist);
        if (desert_edge < 0.99) {
            secondary = 1.0; // Desert
            blend = 1.0 - desert_edge;
        } else if (forest_edge > 0.01) {
            secondary = 2.0; // Forest
            blend = forest_edge;
        } else {
            secondary = 0.0;
            blend = 0.0;
        }
        // Blend with rock near height boundary
        let rock_edge = smoothstep(110.0, 130.0, height);
        if (rock_edge > blend) {
            secondary = 3.0;
            blend = rock_edge;
        }
    }

    // Canonical pair ordering: biome_a <= biome_b
    var a = primary;
    var b = secondary;
    var t = blend;
    if (a > b) {
        let tmp = a;
        a = b;
        b = tmp;
        t = 1.0 - t;
    }

    return vec3<f32>(a, b, t);
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

    let biome = biome_blend(h, m);

    // Write 9 floats per vertex (position, normal, biome_data)
    let base = idx * 9u;
    output[base + 0u] = world_x;
    output[base + 1u] = h;
    output[base + 2u] = world_z;
    output[base + 3u] = normal.x;
    output[base + 4u] = normal.y;
    output[base + 5u] = normal.z;
    output[base + 6u] = biome.x;
    output[base + 7u] = biome.y;
    output[base + 8u] = biome.z;
}

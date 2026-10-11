#include <metal_stdlib>
using namespace metal;

struct CubeVertex {
    float4 position [[position]];
    float4 color;
};

vertex CubeVertex cube_vertex(uint vertex_id [[vertex_id]], constant float4 &parameters [[buffer(0)]]) {
    const float3 corners[] = {
        {-0.6, -0.6, -0.6}, {0.6, -0.6, -0.6}, {0.6, 0.6, -0.6}, {-0.6, 0.6, -0.6},
        {-0.6, -0.6, 0.6}, {0.6, -0.6, 0.6}, {0.6, 0.6, 0.6}, {-0.6, 0.6, 0.6},
    };
    const uint indices[] = {
        0, 1, 2, 0, 2, 3,
        4, 6, 5, 4, 7, 6,
        0, 4, 5, 0, 5, 1,
        3, 2, 6, 3, 6, 7,
        0, 3, 7, 0, 7, 4,
        1, 5, 6, 1, 6, 2,
    };
    const float3 colors[] = {
        {0.2, 0.65, 0.95}, {0.9, 0.2, 0.2}, {0.2, 0.4, 0.55},
        {0.4, 0.85, 0.65}, {0.55, 0.4, 0.8}, {0.85, 0.7, 0.3},
    };
    float3 position = corners[indices[vertex_id]];
    float angle = parameters.x;
    position.xz = float2(position.x * cos(angle) + position.z * sin(angle),
                         -position.x * sin(angle) + position.z * cos(angle));
    position.yz = float2(position.y * cos(0.35) - position.z * sin(0.35),
                         position.y * sin(0.35) + position.z * cos(0.35));
    float opacity = parameters.z;
    return {
        float4(position.x / parameters.y, position.y, 0.5 + position.z * 0.25, 1.0),
        float4(colors[vertex_id / 6] * opacity, opacity),
    };
}

fragment float4 cube_fragment(CubeVertex input [[stage_in]]) {
    return input.color;
}

struct CompositeVertex {
    float4 position [[position]];
    float2 uv;
};

vertex CompositeVertex composite_vertex(uint vertex_id [[vertex_id]]) {
    const float2 positions[] = {
        {-1, -1}, {1, -1}, {-1, 1}, {-1, 1}, {1, -1}, {1, 1},
    };
    float2 position = positions[vertex_id];
    return {float4(position, 0, 1), float2((position.x + 1) * 0.5, (1 - position.y) * 0.5)};
}

fragment float4 composite_fragment(CompositeVertex input [[stage_in]],
                                   texture2d<float> color [[texture(0)]],
                                   constant float &opacity [[buffer(0)]]) {
    constexpr sampler texture_sampler(coord::normalized, address::clamp_to_edge, filter::linear);
    return color.sample(texture_sampler, input.uv) * opacity;
}

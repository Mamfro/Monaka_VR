// An eye shown another way: a full-screen triangle reading the eye's image (and the other eye's,
// and its depth) and writing the swapchain image in its place. The eye's alpha is kept.

Texture2D<float4> eye_image : register(t0);
Texture2D<float4> other_image : register(t1);
Texture2D<float> depth_image : register(t2);
Texture2D<float4> game_buffer : register(t3);
SamplerState nearest : register(s0);

cbuffer View : register(b0) {
    uint mode;          // 1 depth, 2 depth edges, 3 frame age, 4 difference, 5 a game buffer
    uint age;           // presents this eye is older than the other
    uint encoding;      // 1: decode sRGB before an sRGB target, 2: encode sRGB before a plain one
    float range;        // the distance (metres) the depth ramp goes black at
    float4 depth_rect;  // where the eye lies in the depth texture (uv: left, top, width, height)
    float inverse_at_0; // 1 / distance at depth 0, and at depth 1
    float inverse_at_1;
    float2 depth_step;  // one depth texel, in the eye's uv
    uint logarithmic;   // 1: the depth ramp in even steps per doubling of distance
    uint3 unused;
};

struct Out {
    float4 position : SV_Position;
    float2 uv : TEXCOORD;
};

Out vs_main(uint vertex : SV_VertexID) {
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    Out o;
    o.position = float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
    o.uv = uv;
    return o;
}

// Distance (metres) where the eye sees `uv`: 1/distance runs straight across depth values.
float distance_at(float2 uv) {
    float d = depth_image.SampleLevel(nearest, depth_rect.xy + saturate(uv) * depth_rect.zw, 0);
    return 1.0 / max(lerp(inverse_at_0, inverse_at_1, d), 1e-6);
}

// White at the eye, black at the range: in even steps of distance, or (logarithmic) of its
// doublings from 10 cm, which shows near and far detail at once.
float grey_of(float distance) {
    if (logarithmic != 0) {
        return 1 - saturate(log(max(distance, 0.1) / 0.1) / log(range / 0.1));
    }
    return 1 - saturate(distance / range);
}

float3 linear_of(float3 c) {
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

float3 srgb_of(float3 c) {
    return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1 / 2.4) - 0.055;
}

float3 shown(float3 c) {
    if (encoding == 1) return linear_of(c);
    if (encoding == 2) return srgb_of(c);
    return c;
}

float4 ps_main(Out i) : SV_Target {
    float4 c = eye_image.SampleLevel(nearest, i.uv, 0);
    float3 rgb = c.rgb;
    if (mode == 1) {
        float g = grey_of(distance_at(i.uv));
        rgb = float3(g, g, g);
    } else if (mode == 2) {
        float here = log(distance_at(i.uv));
        float across = abs(log(distance_at(i.uv + float2(depth_step.x, 0))) - here);
        float down = abs(log(distance_at(i.uv + float2(0, depth_step.y))) - here);
        float edge = saturate(max(across, down) * 10 - 0.3);
        rgb = lerp(c.rgb, float3(1.0, 0.5, 0.1), edge);
    } else if (mode == 3) {
        // The newer eye as it is; one present older blue, two or more red: strong enough to see
        // in one eye of a fused pair.
        float3 tint = age == 1 ? float3(0.15, 0.35, 1.0) : float3(1.0, 0.15, 0.1);
        rgb = age == 0 ? c.rgb : lerp(c.rgb, tint, 0.45);
    } else if (mode == 5) {
        rgb = game_buffer.SampleLevel(nearest, i.uv, 0).rgb;
    } else if (mode == 4) {
        float3 other = other_image.SampleLevel(nearest, i.uv, 0).rgb;
        rgb = saturate(abs(c.rgb - other) * 4);
    }
    return float4(shown(rgb), c.a);
}

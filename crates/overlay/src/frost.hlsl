// The game blurred behind the panels of the headset's home, so the panels over it read as frosted
// glass: the eye made small (a quarter each way), blurred across and then down, and drawn back into
// the eye inside each panel's rounded rectangle, as that eye saw it.

Texture2D image : register(t0);
SamplerState smooth : register(s0);

cbuffer Frost : register(b0) {
    float4 view0;       // tracking to eye space, rows
    float4 view1;
    float4 view2;
    float4 tangents;    // the eye image's edges: left, right, up, down
    float4 corners[4];  // the panel's: top left, top right, bottom left, bottom right (tracking space)
    float4 texels;      // xy: one texel of the image read (uv); zw: one tap of the blur (uv)
    float4 target;      // xy: the eye image's size (pixels)
    float4 panel;       // xy: the panel's size, z: its corners' radius (the panel's own pixels)
    uint4 mode;         // x: 0 the values go out as read, 1 encoded to sRGB, 2 decoded to linear
};

// The blur's spread and reach, in small texels (a small texel is four of the eye's).
static const float SIGMA = 5.0;
static const int REACH = 12;

struct Full {
    float4 position : SV_Position;
    float2 uv : TEXCOORD;
};

// One triangle over the viewport.
Full vs_full(uint id : SV_VertexID) {
    Full o;
    o.uv = float2((id << 1) & 2, id & 2);
    o.position = float4(o.uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

// Four filtered reads a texel apart: the average of the 4x4 eye texels under a small one.
float4 ps_down(Full i) : SV_Target {
    float2 d = texels.xy;
    float3 c = image.Sample(smooth, i.uv + float2(-d.x, -d.y)).rgb + image.Sample(smooth, i.uv + float2(d.x, -d.y)).rgb
             + image.Sample(smooth, i.uv + float2(-d.x, d.y)).rgb + image.Sample(smooth, i.uv + float2(d.x, d.y)).rgb;
    return float4(c * 0.25, 1.0);
}

// A gaussian along `texels.zw`.
float4 ps_blur(Full i) : SV_Target {
    float3 sum = 0;
    float weights = 0;
    [unroll]
    for (int k = -REACH; k <= REACH; k++) {
        float w = exp(-(k * k) / (2.0 * SIGMA * SIGMA));
        sum += image.Sample(smooth, i.uv + texels.zw * k).rgb * w;
        weights += w;
    }
    return float4(sum / weights, 1.0);
}

struct Panel {
    float4 position : SV_Position;
    float2 uv : TEXCOORD;
};

static const uint ORDER[6] = { 0, 1, 2, 2, 1, 3 };
static const float2 UVS[4] = { float2(0, 0), float2(1, 0), float2(0, 1), float2(1, 1) };

Panel vs_panel(uint vertex : SV_VertexID) {
    uint corner = ORDER[vertex];
    float4 p = float4(corners[corner].xyz, 1);
    float3 v = float3(dot(view0, p), dot(view1, p), dot(view2, p));
    float w = -v.z;
    // Clip space with w = distance ahead, so the panel is mapped in perspective.
    float x = (2 * v.x - (tangents.x + tangents.y) * w) / (tangents.y - tangents.x);
    float y = (2 * v.y - (tangents.z + tangents.w) * w) / (tangents.z - tangents.w);
    Panel o;
    o.position = float4(x, y, 0.5 * w, w);
    o.uv = UVS[corner];
    return o;
}

float3 to_srgb(float3 c) {
    return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

float3 to_linear(float3 c) {
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

// The blurred eye where this pixel is, inside the panel's rounded rectangle (its edge smoothed
// over a pixel).
float4 ps_panel(Panel i) : SV_Target {
    float2 half_size = panel.xy * 0.5;
    float2 q = abs(i.uv * panel.xy - half_size) - (half_size - panel.z);
    float outside = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - panel.z;
    float inside = saturate(0.5 - outside / max(fwidth(outside), 1e-4));
    float3 c = saturate(image.Sample(smooth, i.position.xy / target.xy).rgb);
    c = mode.x == 1 ? to_srgb(c) : mode.x == 2 ? to_linear(c) : c;
    return float4(c * inside, inside);
}

// egui's triangles: positions in points from the top left, colours and textures premultiplied
// sRGB. Blended as they are; into an sRGB target the result is made linear first, so the target's
// encoding gives the same values back.

cbuffer Screen : register(b0) {
    float2 size;       // points
    float linearize;   // 1 for an sRGB target
    float unused;
};

Texture2D picture : register(t0);
SamplerState smooth : register(s0);

struct In {
    float2 position : POSITION;
    float2 uv : TEXCOORD;
    float4 colour : COLOR;
};

struct Out {
    float4 position : SV_Position;
    float2 uv : TEXCOORD;
    float4 colour : COLOR;
};

Out vs_main(In i) {
    Out o;
    o.position = float4(i.position.x / size.x * 2 - 1, 1 - i.position.y / size.y * 2, 0, 1);
    o.uv = i.uv;
    o.colour = i.colour;
    return o;
}

float3 linear_of(float3 c) {
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

float4 ps_main(Out i) : SV_Target {
    float4 c = i.colour * picture.Sample(smooth, i.uv);
    if (linearize > 0.5 && c.a > 0) {
        c.rgb = linear_of(c.rgb / c.a) * c.a;
    }
    return c;
}

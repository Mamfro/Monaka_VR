// A HUD layer laid over an eye image. The HUD was drawn twice, onto black and onto white
// (difference matting, FromMatte in the shared hud.hlsl). The layer is the game's own full-screen
// layout; each eye shows it in Rect (x, y, width, height in pixels), its place for that eye.
cbuffer Composite : register(b0) {
    uint W, H, LW, LH;
    float4 Rect;
};
Texture2D<float4> Base : register(t0);
Texture2D<float4> Black : register(t1);
Texture2D<float4> White : register(t2);
RWTexture2D<unorm float4> Out : register(u0);

[numthreads(8, 8, 1)] void Composite(uint3 id : SV_DispatchThreadID) {
    if (id.x >= W || id.y >= H) return;
    float4 c = Base.Load(int3(id.xy, 0));
    float2 s = LayerPixel(float2(id.xy) + 0.5, Rect, float2(LW, LH));
    if (InLayer(s)) {
        int3 p = int3(s, 0);
        c.rgb = saturate(OverPremultiplied(c.rgb, FromMatte(Black.Load(p).rgb, White.Load(p).rgb)));
    }
    // The alpha as it was: the headset ignores it, and a warped eye carries where each pixel came
    // from there (reproject.hlsl), for captures.
    Out[id.xy] = c;
}

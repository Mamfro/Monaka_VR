// The HUD lay-over's shared functions (hud.rs), put before each compute shader that lays a HUD layer
// over an eye. A layer is the game's own full-screen HUD layout; an eye shows it in a rectangle
// (x, y, width, height in eye pixels) from the placement in hud.rs.
// The layer pixel the eye pixel p (its centre: id + 0.5) shows, the layer being layerSize pixels; a
// negative x outside the rectangle.
float2 LayerPixel(float2 p, float4 rect, float2 layerSize) {
    float2 uv = (p - rect.xy) / rect.zw;
    return (all(uv >= 0) && all(uv < 1)) ? uv * layerSize : float2(-1, -1);
}
bool InLayer(float2 s) { return s.x >= 0; }
// A premultiplied layer pixel u over the colour c.
float3 OverPremultiplied(float3 c, float4 u) { return c * (1 - u.a) + u.rgb; }
// A layer drawn twice, onto black and onto white (difference matting): its coverage is
// 1 - (white - black) and its premultiplied colour the black draw, whatever blend the game used.
float4 FromMatte(float3 black, float3 white) { return float4(black, saturate(1 - dot(white - black, float3(1, 1, 1) / 3))); }
// The world under a premultiplied layer pixel u that the game composited as c = world * (1 - a) + u.
float3 Unblend(float3 c, float4 u) { return saturate((c - u.rgb) / (1 - u.a)); }

// The HUD around frame generation (hud12.rs). The game composites its premultiplied UI layer over
// the world as frame = world * (1 - alpha) + ui.
cbuffer Size:register(b0){uint W,H;};
Texture2D<float4> Ui:register(t0);
Texture2D<float4> Frame:register(t1);
RWTexture2D<unorm float4> Out:register(u0);

// The world without the HUD, recovered where the HUD lets any of it through; under solid HUD the
// frame stays (the fresh HUD laid over the generated frame covers it).
[numthreads(8,8,1)] void Unblend(uint3 id:SV_DispatchThreadID){
    if(id.x>=W || id.y>=H)return;
    float4 u=Ui.Load(int3(id.xy,0));
    float4 c=Frame.Load(int3(id.xy,0));
    if(u.a<0.98)c.rgb=Unblend(c.rgb,u);
    Out[id.xy]=c;}

// The UI layer laid over a frame, in place.
[numthreads(8,8,1)] void Compose(uint3 id:SV_DispatchThreadID){
    if(id.x>=W || id.y>=H)return;
    float4 u=Ui.Load(int3(id.xy,0));
    if(u.a<=0.0 && max(u.r,max(u.g,u.b))<=0.0)return;
    float4 c=Out[id.xy];
    c.rgb=OverPremultiplied(c.rgb,u);
    Out[id.xy]=c;}

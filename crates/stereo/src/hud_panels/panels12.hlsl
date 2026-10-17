// The dynamic HUD's panels from the game's HUD layer (d3d12.rs): each piece's rectangle of the layer
// scaled into its panel (Crop, a 2x2 average); a panel that went away is cleared once (Clear). The layer
// is premultiplied RGBA (a layer keeping what shows through in its alpha is turned round as it is read),
// so averaging is enough. The pieces are kept out of the eyes by whatever lays the layer over them,
// which is told their rectangles.
cbuffer Panel:register(b0){uint OutW,OutH,UiW,UiH;float SrcX,SrcY,SrcW,SrcH;uint SeeThrough,Pad0,Pad1,Pad2;};
Texture2D<float4> Ui:register(t0);
RWTexture2D<unorm float4> Out:register(u0);
// A layer pixel as premultiplied colour and coverage (a layer keeping what shows through in its alpha turned round); none outside it.
float4 Tap(float2 s){int2 p=int2(floor(s));if(!(p.x>=0 && p.y>=0 && p.x<int(UiW) && p.y<int(UiH)))return float4(0,0,0,0);float4 u=Ui.Load(int3(p,0));if(SeeThrough!=0)u.a=1-u.a;return u;}
[numthreads(8,8,1)] void Crop(uint3 id:SV_DispatchThreadID){
    if(id.x>=OutW || id.y>=OutH)return;
    float2 s=float2(SrcX,SrcY)+(float2(id.xy)+0.5)*float2(SrcW,SrcH)/float2(OutW,OutH);
    Out[id.xy]=(Tap(s+float2(-0.5,-0.5))+Tap(s+float2(0.5,-0.5))+Tap(s+float2(-0.5,0.5))+Tap(s+float2(0.5,0.5)))*0.25;}
[numthreads(8,8,1)] void Clear(uint3 id:SV_DispatchThreadID){if(id.x<OutW && id.y<OutH)Out[id.xy]=float4(0,0,0,0);}

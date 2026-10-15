use super::*;
use monaka_channel::d3d::{create_device, read_pixel};
use windows::core::s;

/// Writes its constants (bytes) to every texel of its target.
const FILL: &str = "cbuffer Fill : register(b0) { uint4 V; };
RWTexture2D<unorm float4> Out : register(u0);
[numthreads(8, 8, 1)] void Main(uint3 id : SV_DispatchThreadID) { Out[id.xy] = V / 255.0; }";

#[test]
fn constants_reach_a_written_texture() {
    let (device, context, _) = create_device(None).unwrap();
    let shader = compute_shader(&device, FILL, "fill", s!("Main")).unwrap();
    let constants = ConstantBuffer::<[u32; 4]>::new(&device).unwrap();
    let (texture, uav) = uav_texture(&device, 8, 8, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    shader_view(&device, &texture, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
    for value in [[10, 20, 30, 255], [200, 100, 50, 25]] {
        constants.update(&context, &value);
        constants.bind(&context);
        // SAFETY: test dispatch on our own device's context.
        unsafe {
            context.CSSetShader(&shader, None);
            context.CSSetUnorderedAccessViews(0, 1, Some([Some(uav.clone())].as_ptr()), None);
            context.Dispatch(1, 1, 1);
            context.CSSetUnorderedAccessViews(0, 1, Some([None].as_ptr()), None);
        }
        assert_eq!(read_pixel(&device, &context, &texture, 5, 6).unwrap(), value.map(|v| v as u8));
    }
}

#[test]
fn compile_errors_name_the_shader() {
    let Err(Error::Shader(message)) = compile("not hlsl", "broken.hlsl", s!("Main")) else { panic!("compiled") };
    assert!(message.contains("broken.hlsl Main"), "{message}");
}

//! The dynamic HUD's layer crops on D3D12, for a game whose HUD is a premultiplied RGBA layer of its
//! own (the HUD layer of `crate::providers::HudLayer12`, or one the game tags for frame generation):
//! at each present, before the layer goes over the eyes, the game's boxes are taken
//! ([`super::LayerPieces::boxes`]), each shown piece's rectangle of the layer is scaled into its
//! panel (a fixed-size RGBA texture, a 2x2 average) and published on its own fence channel,
//! `<channel>-panel-<piece>`; a panel no longer shown is published cleared, once. The rectangles
//! go back to the caller, to be left out of the HUD laid over the eyes ([`publish`]) or taken out
//! of an eye frame that has the HUD drawn in ([`take_out`]). The shaders are `panels12.hlsl` and
//! `take_out12.hlsl`.

pub(super) mod take_out;

pub use take_out::take_out;

use super::{PANEL_PIECES, PanelOut, Rects};
use crate::framepass12::Lazy;
use monaka_channel::ChannelName;
use monaka_channel::d3d12::compute::{self, DescriptorHeap, Table, srv_desc, uav_desc};
use monaka_channel::d3d12::{self, Recorder, barrier};
use monaka_channel::fence::FenceProducer;
use monaka_producer::log;
use std::ffi::c_void;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::core::s;

const SOURCE: &str = include_str!("panels12.hlsl");

/// The crop shader's constants (root constants).
#[repr(C)]
#[derive(Clone, Copy)]
struct Constants {
    out_width: u32,
    out_height: u32,
    ui_width: u32,
    ui_height: u32,
    source: [f32; 4],
    /// The layer's alpha is what shows through ([`super::LayerPieces::SEE_THROUGH`]).
    see_through: u32,
    pad: [u32; 3],
}

const CONSTANTS: u32 = (size_of::<Constants>() / 4) as u32;

struct Panel {
    name: &'static str,
    size: (u32, u32),
    texture: ID3D12Resource,
    producer: Option<FenceProducer>,
    failures: u64,
}

/// What goes out for each piece at a present (shown, just cleared, or nothing).
type Outs = [Option<PanelOut>; PANEL_PIECES];

struct Panels {
    device: ID3D12Device,
    root: ID3D12RootSignature,
    crop: ID3D12PipelineState,
    clear: ID3D12PipelineState,
    heap: DescriptorHeap,
    recorder: Recorder,
    panels: Vec<Panel>,
    /// Which panels went out shown.
    states: super::PanelStates,
    /// The layer the heap's views were made for.
    viewed: Option<ID3D12Resource>,
    published: u64,
}

static PANELS: Lazy<Panels> = Lazy::new();

impl Panels {
    fn new(device: &ID3D12Device, pieces: &[super::Piece]) -> Result<Self, String> {
        // Constants, then the layer (t0), then one panel (u0).
        let root = compute::root_signature(device, CONSTANTS, &[Table { srvs: 1, uavs: 0 }, Table { srvs: 0, uavs: 1 }])?;
        let pipeline = |entry| compute::shader_pipeline(device, &root, SOURCE, "panels12.hlsl", entry);
        let (crop, clear) = (pipeline(s!("Crop"))?, pipeline(s!("Clear"))?);
        let heap = DescriptorHeap::new(device, 1 + pieces.len().max(1) as u32)?;
        let recorder = Recorder::new(device, 4).map_err(|e| format!("command lists: {e}"))?;
        let mut panels = Vec::new();
        for piece in pieces {
            let (name, size) = (piece.name, piece.size);
            let texture = d3d12::texture(device, size.0, size.1, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).map_err(|e| format!("panel {name}: {e}"))?;
            panels.push(Panel { name, size, texture, producer: None, failures: 0 });
        }
        Ok(Self { device: device.clone(), root, crop, clear, heap, recorder, panels, states: super::PanelStates::default(), viewed: None, published: 0 })
    }

    /// Views: the layer's SRV at 0, then each panel's UAV.
    fn view(&mut self, layer: &ID3D12Resource) {
        if self.viewed.as_ref() == Some(layer) {
            return;
        }
        let rgba = DXGI_FORMAT_R8G8B8A8_UNORM;
        // SAFETY: views of live textures into our own heap.
        unsafe {
            self.device.CreateShaderResourceView(layer, Some(&srv_desc(crate::framepass12::ui_view_format(layer.GetDesc().Format))), self.heap.cpu(0));
            for (i, panel) in self.panels.iter().enumerate() {
                self.device.CreateUnorderedAccessView(&panel.texture, None, Some(&uav_desc(rgba)), self.heap.cpu(1 + i as u32));
            }
        }
        self.viewed = Some(layer.clone());
    }

    /// Records this present's panels from `layer` (in COMMON, of `size`; its alpha what shows
    /// through when `see_through`) for the shown pieces' rectangles `rects` (by piece, the layer's
    /// pixels): which panels to publish and how (shown ones and ones just cleared), by piece.
    /// `None` when no command list was free.
    fn record(&mut self, layer: &ID3D12Resource, size: (u32, u32), see_through: bool, rects: &[Option<[f32; 4]>; PANEL_PIECES]) -> windows::core::Result<Option<Outs>> {
        let Some(list) = self.recorder.begin()? else { return Ok(None) };
        self.view(layer);
        let mut outs: Outs = [None; PANEL_PIECES];
        for (i, out) in outs.iter_mut().enumerate().take(self.panels.len()) {
            let rect = rects[i];
            *out = match (rect, self.states.out(i, rect.is_some())) {
                (Some(_), _) => Some(PanelOut::Shown),
                (None, Some(PanelOut::Cleared)) => Some(PanelOut::Cleared),
                _ => None,
            };
        }
        let cropped = || outs.iter().enumerate().filter(|(_, out)| **out == Some(PanelOut::Shown)).filter_map(|(i, _)| Some((i, rects[i]?)));
        let cleared = || outs.iter().enumerate().filter(|(_, out)| **out == Some(PanelOut::Cleared)).map(|(i, _)| i);
        let touched = || outs.iter().enumerate().filter(|(_, out)| out.is_some()).map(|(i, _)| i);
        const SRV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
        let constants = |panel: &Panel, source: [f32; 4]| Constants {
            out_width: panel.size.0,
            out_height: panel.size.1,
            ui_width: size.0,
            ui_height: size.1,
            source,
            see_through: u32::from(see_through),
            pad: [0; 3],
        };
        if touched().next().is_none() {
            return Ok(Some(outs));
        }
        // SAFETY: records compute work on our own list with our own heap, root signature and
        // pipelines; every resource is live until the list has executed (the recorder's fence).
        unsafe {
            list.SetDescriptorHeaps(&[Some(self.heap.heap().clone())]);
            list.SetComputeRootSignature(&self.root);
            list.SetComputeRootDescriptorTable(1, self.heap.gpu(0));
            for i in touched() {
                barrier(&list, &self.panels[i].texture, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
            }
            barrier(&list, layer, D3D12_RESOURCE_STATE_COMMON, SRV);
            list.SetPipelineState(&self.crop);
            for (i, rect) in cropped() {
                let panel = &self.panels[i];
                let c = constants(panel, rect);
                list.SetComputeRoot32BitConstants(0, CONSTANTS, &c as *const Constants as *const c_void, 0);
                list.SetComputeRootDescriptorTable(2, self.heap.gpu(1 + i as u32));
                list.Dispatch(panel.size.0.div_ceil(8), panel.size.1.div_ceil(8), 1);
            }
            barrier(&list, layer, SRV, D3D12_RESOURCE_STATE_COMMON);
            list.SetPipelineState(&self.clear);
            for i in cleared() {
                let panel = &self.panels[i];
                let c = constants(panel, [0.0; 4]);
                list.SetComputeRoot32BitConstants(0, CONSTANTS, &c as *const Constants as *const c_void, 0);
                list.SetComputeRootDescriptorTable(2, self.heap.gpu(1 + i as u32));
                list.Dispatch(panel.size.0.div_ceil(8), panel.size.1.div_ceil(8), 1);
            }
            for i in touched() {
                barrier(&list, &self.panels[i].texture, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON);
            }
        }
        Ok(Some(outs))
    }

    /// Publishes the panels `touched` (each shown or cleared) on `channel`'s panel channels (each
    /// made at its first publish).
    fn publish(&mut self, queue: &ID3D12CommandQueue, channel: &ChannelName, touched: Outs, tick: u64) {
        let device = self.device.clone();
        for (i, out) in touched.into_iter().enumerate().filter_map(|(i, out)| Some((i, out?))) {
            let panel = &mut self.panels[i];
            if panel.producer.is_none() {
                let made = channel.panel(panel.name).ok_or_else(|| "bad channel name".to_owned()).and_then(|name| {
                    FenceProducer::create_mono(&name, &device, queue, panel.size.0, panel.size.1, DXGI_FORMAT_R8G8B8A8_UNORM).map_err(|e| e.to_string())
                });
                match made {
                    Ok(producer) => {
                        log!("dynamic HUD: panel {} published on its channel ({}x{})", panel.name, panel.size.0, panel.size.1);
                        panel.producer = Some(producer);
                    }
                    Err(why) => {
                        panel.failures += 1;
                        if panel.failures <= 3 {
                            log!("dynamic HUD: panel {} has no channel: {why}", panel.name);
                        }
                        continue;
                    }
                }
            }
            let Some(producer) = panel.producer.as_mut() else { continue };
            match producer.publish_one((&panel.texture, D3D12_RESOURCE_STATE_COMMON), tick) {
                Ok(true) => {
                    self.states.went_out(i, out);
                    self.published += 1;
                }
                Ok(false) => {}
                Err(e) => {
                    panel.failures += 1;
                    if panel.failures <= 3 {
                        log!("dynamic HUD: panel {} publish failed: {e}", panel.name);
                    }
                }
            }
        }
    }
}

/// At a present, before the HUD layer `layer` (in COMMON) goes over the eyes: the pieces where the
/// game says they lie now, each shown piece's panel cut out of the layer and published. Returns the
/// shown pieces' rectangles of the layer (x, y, width, height in its pixels), to be left out of
/// the eyes; none while the dynamic HUD is not taking pieces.
pub fn publish(queue: &ID3D12CommandQueue, layer: &ID3D12Resource) -> Rects {
    if !super::active() {
        return Rects::default();
    }
    // SAFETY: reads a resource descriptor.
    let desc = unsafe { layer.GetDesc() };
    let size = (desc.Width as u32, desc.Height);
    super::take_boxes(size);
    let Some(frame) = super::frame() else { return Rects::default() };
    let rects = frame.rects.map(|r| r.map(|r| super::pixels(r, size)));
    let make = |device: &ID3D12Device| {
        let made = Panels::new(device, frame.pieces)?;
        log!("dynamic HUD: layer crops ready ({} pieces)", made.panels.len());
        Ok(made)
    };
    let recorded = PANELS.with(queue, make, "dynamic HUD off", |panels| {
        let touched = match panels.record(layer, size, frame.see_through, &rects) {
            Ok(Some(touched)) => touched,
            Ok(None) => return false,
            Err(e) => {
                monaka_producer::log_first(3, format_args!("dynamic HUD: panels not recorded: {e}"));
                return false;
            }
        };
        if let Err(e) = panels.recorder.submit(queue) {
            monaka_producer::log_first(3, format_args!("dynamic HUD: panels not submitted: {e}"));
            return false;
        }
        panels.publish(queue, &frame.channel, touched, monaka_channel::tick());
        true
    });
    if recorded == Some(true) { Rects::of(rects) } else { Rects::default() }
}

/// The end of a run: the channels go away with the producers, once the GPU is done with the panels.
pub(super) fn release() {
    if let Some(made) = PANELS.take() {
        made.recorder.idle(2000);
        log!("dynamic HUD: {} panels published", made.published);
    }
    take_out::release();
}

#[cfg(test)]
pub(super) fn made(device: &ID3D12Device, pieces: &[super::Piece]) -> Result<usize, String> {
    Panels::new(device, pieces).map(|p| p.panels.len())
}

#[cfg(test)]
pub(super) fn compiles() -> Result<(), String> {
    for entry in [s!("Crop"), s!("Clear")] {
        compute::compile(SOURCE, "panels12.hlsl", entry)?;
    }
    Ok(())
}

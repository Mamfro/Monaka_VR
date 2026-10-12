//! The shading-rate images: each eye's, rewritten when its gaze moves to another tile, written into
//! an upload buffer and copied on the game's own list before the pass that uses it.

use super::*;

/// Whether the game's device has VRS tier 2: its tile size and whether it has the coarser rates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Support {
    Unknown,
    No,
    Yes { tile: u32, coarse: bool },
}

/// What an eye's image holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Key {
    pub(super) frustum: [f32; 4],
    pub(super) centre: (u32, u32),
    pub(super) rings: Rings,
}

/// Upload slots in the ring: a slot is written again only this many image writes later, long
/// after the GPU copied it.
pub(super) const SLOTS: u64 = 32;

/// Both eyes' images for one render size, and the upload ring they are written through.
pub(super) struct Images {
    pub(super) pixels: (u32, u32),
    pub(super) tile: u32,
    pub(super) eyes: [ID3D12Resource; 2],
    pub(super) holds: [Option<Key>; 2],
    pub(super) upload: ID3D12Resource,
    pub(super) mapped: *mut u8,
    pub(super) pitch: u32,
    pub(super) slot_bytes: u64,
    pub(super) next: u64,
    pub(super) generation: u64,
}

// SAFETY: the D3D objects are free-threaded, and the mapped upload memory is written only under the
// state's mutex.
unsafe impl Send for Images {}

impl Images {
    pub(super) fn new(device: &ID3D12Device, pixels: (u32, u32), tile: u32, generation: u64) -> Result<Self, String> {
        let (across, down) = tiles(pixels, tile);
        let eyes = [image(device, across, down)?, image(device, across, down)?];
        let pitch = across.next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT);
        let slot_bytes = (pitch as u64 * down as u64).next_multiple_of(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT as u64);
        let upload = upload_buffer(device, slot_bytes * SLOTS)?;
        let mut mapped = std::ptr::null_mut();
        // SAFETY: maps our own upload buffer for writing, kept mapped for its life.
        unsafe { upload.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut mapped)) }.map_err(|e| format!("upload map: {e}"))?;
        Ok(Self { pixels, tile, eyes, holds: [None; 2], upload, mapped: mapped.cast(), pitch, slot_bytes, next: 0, generation })
    }

    /// Writes eye `eye`'s image as `key` says, the copy recorded on `list`.
    pub(super) fn write(&mut self, list: &ID3D12GraphicsCommandList, eye: usize, key: &Key) {
        let offset = (self.next % SLOTS) * self.slot_bytes;
        self.next += 1;
        // SAFETY: a slot of our own mapped upload buffer, within its size.
        let data = unsafe { std::slice::from_raw_parts_mut(self.mapped.add(offset as usize), self.slot_bytes as usize) };
        let [left, right, up, down] = key.frustum;
        fill(&Frustum { left, right, up, down }, self.pixels, self.tile, key.centre, &key.rings, data, self.pitch as usize);
        let (across, rows) = tiles(self.pixels, self.tile);
        let footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
            Offset: offset,
            Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: DXGI_FORMAT_R8_UINT, Width: across, Height: rows, Depth: 1, RowPitch: self.pitch },
        };
        let target = &self.eyes[eye];
        barrier(list, target, D3D12_RESOURCE_STATE_SHADING_RATE_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        // SAFETY: records a copy between our own live resources on the game's list being recorded.
        unsafe { list.CopyTextureRegion(&subresource(target, 0), 0, 0, 0, &placed(&self.upload, footprint), None) };
        barrier(list, target, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_SHADING_RATE_SOURCE);
        self.holds[eye] = Some(*key);
    }
}

/// A shading-rate image of `across` x `down` tiles, kept in the shading-rate source state (it is
/// all 1x1 until written: a new texture is zeroed).
pub(super) fn image(device: &ID3D12Device, across: u32, down: u32) -> Result<ID3D12Resource, String> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: across as u64,
        Height: down,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8_UINT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    let mut out: Option<ID3D12Resource> = None;
    // SAFETY: creation with a valid descriptor into a local out-pointer.
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_SHADING_RATE_SOURCE, None, &mut out) }.map_err(|e| format!("shading-rate image: {e}"))?;
    out.ok_or_else(|| "shading-rate image: none made".to_owned())
}

pub(super) fn upload_buffer(device: &ID3D12Device, bytes: u64) -> Result<ID3D12Resource, String> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut out: Option<ID3D12Resource> = None;
    // SAFETY: creation with a valid descriptor into a local out-pointer.
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut out) }.map_err(|e| format!("upload buffer: {e}"))?;
    out.ok_or_else(|| "upload buffer: none made".to_owned())
}

pub(super) struct State {
    pub(super) game: Option<GameFns>,
    /// The game's device (its queue's), for its VRS support and the images.
    pub(super) device: Option<ID3D12Device>,
    pub(super) rings: Rings,
    pub(super) support: Support,
    pub(super) images: Option<Images>,
    /// Images of an earlier size, kept: frames in flight may still read them.
    pub(super) retired: Vec<Images>,
    /// The present of the last frame VRS was counted for.
    pub(super) counted: Option<u64>,
}

pub(super) static STATE: Mutex<State> = Mutex::new(State {
    game: None,
    device: None,
    rings: Rings { inner: 0.0, middle: 0.0, outer: rate::R2X2 },
    support: Support::Unknown,
    images: None,
    retired: Vec::new(),
    counted: None,
});

/// Retired image sets kept at most (the oldest let go: long done on the GPU).
pub(super) const RETIRED_KEPT: usize = 4;

impl State {
    /// The game device's VRS support, asked once.
    pub(super) fn support(&mut self) -> Option<(u32, bool)> {
        if self.support == Support::Unknown {
            self.support = self.device.as_ref().map_or(Support::No, device_support);
        }
        match self.support {
            Support::Yes { tile, coarse } => Some((tile, coarse)),
            _ => None,
        }
    }

    /// The images for a scene of `pixels`, made on the game's device when there are none or they
    /// are of another size (the old ones retired).
    pub(super) fn images(&mut self, pixels: (u32, u32), tile: u32) -> Option<&mut Images> {
        if self.images.as_ref().is_none_or(|i| i.pixels != pixels || i.tile != tile) {
            let generation = self.images.as_ref().map_or(0, |i| i.generation + 1);
            let made = self.device.as_ref().ok_or_else(|| "no device".to_owned()).and_then(|device| Images::new(device, pixels, tile, generation));
            match made {
                Ok(images) => {
                    let (across, down) = tiles(pixels, tile);
                    log!("foveation: shading-rate images of {across}x{down} tiles for {}x{}", pixels.0, pixels.1);
                    if let Some(old) = self.images.replace(images) {
                        if self.retired.len() >= RETIRED_KEPT {
                            self.retired.remove(0);
                        }
                        self.retired.push(old);
                    }
                }
                Err(why) => {
                    log!("foveation off: {why}");
                    self.support = Support::No;
                    return None;
                }
            }
        }
        self.images.as_mut()
    }
}

pub(super) fn device_support(device: &ID3D12Device) -> Support {
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS6::default();
    // SAFETY: asks the live device for its options into a local of the matching type and size.
    let asked = unsafe { device.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS6, &mut options as *mut _ as *mut c_void, size_of_val(&options) as u32) };
    let tier = options.VariableShadingRateTier.0;
    let tile = options.ShadingRateImageTileSize;
    if asked.is_err() || tier < D3D12_VARIABLE_SHADING_RATE_TIER_2.0 || tile == 0 {
        log!("foveation off: the GPU has variable rate shading tier {tier} (tier 2 is needed; asked: {asked:?})");
        return Support::No;
    }
    let coarse = options.AdditionalShadingRatesSupported.as_bool();
    log!("foveation: variable rate shading tier {tier}, {tile}-pixel tiles, {} the coarser rates (2x4, 4x4)", if coarse { "with" } else { "without" });
    Support::Yes { tile, coarse }
}

/// The image to set for a scene pass at `scene`, and what it is (its eye and generation): the
/// frame's eye's, brought up to date first when `may_copy` (the copy recorded on `recording`'s
/// list). `None`: no VRS for this pass.
pub(super) fn image_for(recording: &Recording, scene: (u32, u32), may_copy: bool) -> Option<(ID3D12Resource, (usize, u64))> {
    if DRIVER.flat().is_some() {
        return None;
    }
    let mut state = STATE.locked();
    let game = state.game?;
    let (tile, coarse) = state.support()?;
    let frame = (game.frame)()?;
    let eye = frame.eye.min(1);
    let frustum = (game.frustum)(&frame.pose, eye).or_else(|| Frustum::centred([1.0, 1.0]))?;
    let gaze = live_gaze(&frame.pose);
    let rings = if gaze.is_some() { state.rings } else { state.rings.fixed() };
    let rings = if coarse { rings } else { rings.basic() };
    let centre = tile_of(&frustum, scene, tile, gaze.unwrap_or(AHEAD)).unwrap_or_else(|| {
        let (across, down) = tiles(scene, tile);
        (across / 2, down / 2)
    });
    let key = Key { frustum: [frustum.left, frustum.right, frustum.up, frustum.down], centre, rings };
    let present = DRIVER.presents();
    let counted = state.counted.replace(present) != Some(present);
    let images = state.images(scene, tile)?;
    if may_copy
        && images.holds[eye] != Some(key)
        && let Some(list) = recording.list()
    {
        images.write(list, eye, &key);
        TALLY.writes.fetch_add(1, Relaxed);
    }
    let chosen = (images.eyes[eye].clone(), (eye, images.generation));
    if counted {
        TALLY.frames.fetch_add(1, Relaxed);
        if gaze.is_some() {
            TALLY.gaze_frames.fetch_add(1, Relaxed);
        }
    }
    Some(chosen)
}

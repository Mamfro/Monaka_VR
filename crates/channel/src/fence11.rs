//! The fence channel ([`crate::fence`]) written from a D3D11 game: the same header, slots and
//! shared fences, made with D3D11.4's shareable textures and fences, so the reader cannot tell the
//! producer's API. Unlike the keyed channel it takes the game's own back buffer format (10-bit
//! included), and each eye is copied straight from its region of the back buffer into a slot.
use crate::d3d::{self, adapter_luid};
use crate::shared::{Create, OwnedHandle, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_core::fence_rules::choose_slot;
use monaka_core::protocol::{FENCE_MAGIC, FENCE_SLOTS, FENCE_VERSION, FenceHeader, NO_SLOT, dimensions_ok, fence_format_ok};
use windows::Win32::Foundation::GENERIC_ALL;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_FENCE_FLAG_SHARED, D3D11_RESOURCE_MISC_SHARED,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, ID3D11Device, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIResource1};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::core::{Interface, PCWSTR};

pub struct FenceProducer11 {
    header: SharedBlock<FenceHeader>,
    device: ID3D11Device,
    textures: [[ID3D11Texture2D; 2]; FENCE_SLOTS],
    produced: ID3D11Fence,
    consumed: ID3D11Fence,
    _handles: Vec<OwnedHandle>,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    sequence: u64,
}

impl FenceProducer11 {
    /// Claims `name` and creates the shared slots on `device` (D3D11.4 or later, for shared fences).
    pub fn create(name: &ChannelName, device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Self> {
        if !dimensions_ok(width, height) || !fence_format_ok(format.0 as u32) {
            return Err(Error::Incompatible("eye size or format"));
        }
        let device5: ID3D11Device5 = device.cast()?;
        let adapter = adapter_luid(device)?;
        let mut header = SharedBlock::<FenceHeader>::create(&name.header(), &name.header_lock(), Create::Exclusive)?;
        let mut record = header.lock(0).tolerant().ok_or(Error::Busy)?;
        let mut desc = d3d::simple_texture_desc(width, height, format);
        desc.BindFlags = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
        desc.MiscFlags = (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32;
        let mut handles = Vec::new();
        let mut keep = |handle: windows::Win32::Foundation::HANDLE| -> Result<u64> {
            let value = handle.0 as usize as u64;
            handles.push(OwnedHandle::new(handle).ok_or(Error::Incompatible("shared handle"))?);
            Ok(value)
        };
        let mut published = [[0u64; 2]; FENCE_SLOTS];
        let mut slots = Vec::with_capacity(FENCE_SLOTS);
        for slot in published.iter_mut() {
            let pair = [d3d::create_texture(device, &desc)?, d3d::create_texture(device, &desc)?];
            for (value, texture) in slot.iter_mut().zip(&pair) {
                let resource: IDXGIResource1 = texture.cast()?;
                let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
                // SAFETY: an unnamed NT handle for a shareable texture we own.
                *value = keep(unsafe { resource.CreateSharedHandle(None, access, PCWSTR::null()) }?)?;
            }
            slots.push(pair);
        }
        let fence = || -> Result<ID3D11Fence> {
            let mut fence = None;
            // SAFETY: creates a shareable fence on our device into a local out-pointer.
            unsafe { device5.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence) }?;
            fence.ok_or(Error::Incompatible("shared fence"))
        };
        let (produced, consumed) = (fence()?, fence()?);
        // SAFETY: unnamed NT handles for our own fences.
        let produced_handle = keep(unsafe { produced.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null()) }?)?;
        let consumed_handle = keep(unsafe { consumed.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null()) }?)?;
        *record = FenceHeader {
            magic: FENCE_MAGIC,
            version: FENCE_VERSION,
            width,
            height,
            format: format.0 as u32,
            // SAFETY: no preconditions.
            producer_pid: unsafe { GetCurrentProcessId() },
            adapter,
            textures: published,
            produced: produced_handle,
            consumed: consumed_handle,
            latest_slot: NO_SLOT,
            reader_slot: NO_SLOT,
            ..FenceHeader::default()
        };
        drop(record);
        let textures = slots.try_into().map_err(|_| Error::Incompatible("slots"))?;
        Ok(Self { header, device: device.clone(), textures, produced, consumed, _handles: handles, width, height, format, sequence: 0 })
    }

    /// The newest pair whose copy has finished on the GPU (what a reader can take).
    pub fn completed(&self) -> u64 {
        // SAFETY: reads a fence value.
        unsafe { self.produced.GetCompletedValue() }
    }

    /// The sequence the next successful publish will carry, so its pose record can go out first.
    pub fn next_sequence(&self) -> u64 {
        self.sequence + 1
    }

    /// Copies the two eyes, regions `regions` of `source` (each the channel's size), into a free
    /// slot and publishes them once the copies have run on the GPU. `Ok(false)`: skipped because no
    /// slot was free; it never waits. `context` is the device's immediate context.
    pub fn publish(&mut self, context: &ID3D11DeviceContext, source: &ID3D11Texture2D, regions: [D3D11_BOX; 2], tick: u64) -> Result<bool> {
        self.publish_eyes(context, [(source, regions[0]), (source, regions[1])], tick)
    }

    /// [`FenceProducer11::publish`] with each eye from its own texture: `(texture, region)` for the
    /// left eye, then the right.
    pub fn publish_eyes(&mut self, context: &ID3D11DeviceContext, eyes: [(&ID3D11Texture2D, D3D11_BOX); 2], tick: u64) -> Result<bool> {
        for (source, region) in eyes {
            let desc = d3d::texture_desc(source);
            let fits = region.right - region.left == self.width && region.bottom - region.top == self.height && region.right <= desc.Width && region.bottom <= desc.Height;
            if d3d::texture_device(source).ok().as_ref() != Some(&self.device) || desc.Format != self.format || desc.SampleDesc.Count != 1 || !fits {
                return Err(Error::Incompatible("eye regions"));
            }
        }
        if !d3d::is_immediate_context_of(context, &self.device) {
            return Err(Error::Incompatible("context"));
        }
        let context4: ID3D11DeviceContext4 = context.cast()?;
        let sequence = self.sequence + 1;
        // The slot is marked with this pair before its copy is queued, in the same lock as it is
        // chosen: the reader never takes it while it is being rewritten.
        let slot = {
            let Some(mut record) = self.header.lock(0).tolerant() else { return Ok(false) };
            // SAFETY: reads a fence value.
            let Some(slot) = choose_slot(&record, unsafe { self.consumed.GetCompletedValue() }) else { return Ok(false) };
            let slot = slot as usize;
            (record.slot_sequences[slot], record.slot_ticks[slot]) = (sequence, tick);
            slot
        };
        // SAFETY: in-bounds regions of a same-format texture on our device into our own slot,
        // then the shared fence signalled behind the copies.
        unsafe {
            for (target, (source, region)) in self.textures[slot].iter().zip(&eyes) {
                context.CopySubresourceRegion(target, 0, 0, 0, 0, *source, 0, Some(region));
            }
            context4.Signal(&self.produced, sequence)?;
            context.Flush();
        }
        // Only the producer writes `sequence`; the lock orders it against a reader's claim.
        let mut record = loop {
            if let Some(record) = self.header.lock(0).tolerant() {
                break record;
            }
            std::hint::spin_loop();
        };
        record.latest_slot = slot as u32;
        record.tick = tick;
        record.sequence = sequence;
        self.sequence = sequence;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "needs a GPU"]
    fn a_reader_keeps_up_with_a_steady_producer() {
        use crate::d3d::create_device;
        use crate::fence::FenceReader;
        use std::time::{Duration, Instant};
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
        let name = ChannelName::new(&format!(r"Local\monaka-test-fence11-rate-{}", std::process::id())).unwrap();
        let (width, height) = (1920, 1080);
        let (device, context, _) = create_device(None).unwrap();
        let source = d3d::create_texture(&device, &d3d::simple_texture_desc(width, height, DXGI_FORMAT_B8G8R8A8_UNORM)).unwrap();
        let mut producer = FenceProducer11::create(&name, &device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
        let whole = D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
        // The first pair makes the channel readable.
        assert!(producer.publish(&context, &source, [whole, whole], 0).unwrap());
        let reader_name = name.clone();
        let reader = std::thread::spawn(move || {
            let (device, context, _) = create_device(None).unwrap();
            let mut reader = FenceReader::open(&reader_name, &device).unwrap();
            let targets = [0, 1].map(|_| d3d::create_texture(&device, &d3d::simple_texture_desc(width, height, DXGI_FORMAT_B8G8R8A8_UNORM)).unwrap());
            let (mut seen, mut reads, mut busy) = (0, 0, 0);
            let deadline = Instant::now() + Duration::from_millis(2500);
            while Instant::now() < deadline {
                match reader.read(&context, [&targets[0], &targets[1]], seen).unwrap() {
                    Some(pair) => {
                        seen = pair.sequence;
                        reads += 1;
                    }
                    None => busy += 1,
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            (reads, seen, busy)
        });
        let mut published = 1;
        for i in 0..200 {
            if producer.publish(&context, &source, [whole, whole], i).unwrap() {
                published += 1;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (reads, last, polls_without) = reader.join().unwrap();
        println!("published {published}, read {reads}, last sequence read {last}, polls with nothing new {polls_without}");
        assert!(reads * 10 >= published * 8, "the reader read {reads} of {published}");
    }

    use super::*;
    use crate::d3d::{create_device, create_texture_with, read_pixel};
    use crate::fence::FenceReader;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

    #[test]
    #[ignore = "needs a GPU"]
    fn a_d3d11_producer_reaches_the_d3d11_reader() {
        let (device, context, _) = create_device(None).unwrap();
        // A 64x32 side-by-side image: red on the left, blue on the right.
        let pixels: Vec<u8> = (0..32).flat_map(|_| (0..64).flat_map(|x| if x < 32 { [255, 0, 0, 255] } else { [0, 0, 255, 255] })).collect();
        let source = create_texture_with(&device, &d3d::simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM), &pixels).unwrap();
        let name = ChannelName::new(&format!(r"Local\monaka-test-fence11-{}", std::process::id())).unwrap();
        let mut producer = FenceProducer11::create(&name, &device, 32, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
        let eye = |x: u32| D3D11_BOX { left: x, top: 0, front: 0, right: x + 32, bottom: 32, back: 1 };
        assert!(producer.publish(&context, &source, [eye(0), eye(32)], 7).unwrap());

        let (reader_device, reader_context, _) = create_device(None).unwrap();
        let mut reader = FenceReader::open(&name, &reader_device).unwrap();
        let targets = [0, 1].map(|_| d3d::create_texture(&reader_device, &d3d::simple_texture_desc(32, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        let pair = loop {
            if let Some(pair) = reader.read(&reader_context, [&targets[0], &targets[1]], 0).unwrap() {
                break pair;
            }
            assert!(Instant::now() < deadline, "no pair");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!((pair.sequence, pair.tick), (1, 7));
        assert_eq!(read_pixel(&reader_device, &reader_context, &targets[0], 5, 5).unwrap(), [255, 0, 0, 255]);
        assert_eq!(read_pixel(&reader_device, &reader_context, &targets[1], 5, 5).unwrap(), [0, 0, 255, 255]);
    }
}

//! At the present loop's step: each pair published (AFW alone at once; frame generation's generated
//! pair at once and the real pair half a frame later, paced), a menu flat, a frame the same-frame
//! loop hands over ([`pair_present`]); the stop ([`close`]) and the counts ([`report`]).

use super::*;


/// A pair waiting to be published (frame generation's real pair), with its depth for the headset.
pub(super) struct Due {
    pub(super) images: [ID3D12Resource; 2],
    pub(super) pose: HeadPose,
    pub(super) depth: Option<PairDepth>,
}

pub(super) static PUBLISHER: Mutex<Option<PairPublisher12>> = Mutex::new(None);
pub(super) static PACING: Pacing<Due> = Pacing::new("Monaka VR frame pacing", send_due);

/// Publishes `images` (left, right; each in its state) with `poses` and each eye's `depth`, or flat
/// without either: on the same-frame loop through its pair sink, else on frame warping's own
/// publisher.
pub(super) fn send(images: [(&ID3D12Resource, D3D12_RESOURCE_STATES); 2], poses: Option<[HeadPose; 2]>, depth: Option<&PairDepth>) -> Step {
    let Some(game) = *GAME.locked() else { return Step::Failed };
    if game.on == OnLoop::SameFrame {
        return crate::same_frame::d3d12::sink(images, poses, depth);
    }
    let mut publisher = PUBLISHER.locked();
    let Some(publisher) = publisher.as_mut() else { return Step::Failed };
    let head = game.head();
    match poses {
        Some(poses) => publisher.publish_with_depth(images, depth, poses, head, game.report),
        None => publisher.publish_flat(images, head),
    }
}

/// Both eyes' depth of a warped pair for the headset, while it goes there: the real eye's copy and
/// the warped eye's, each in its own view. Each lines up whole with its image, as the warp made
/// them. They stay valid until that eye's next real frame (the real eye's) and for the warp's ring
/// of pairs (the warped eye's), so also for frame generation's real pair half a frame later.
pub(super) fn pair_depth(pair: &Pair, units_per_metre: f32) -> Option<PairDepth> {
    if !crate::depth_layer::on() {
        return None;
    }
    let params = pair.views[pair.real_eye].depth_params(units_per_metre)?;
    Some(PairDepth { eyes: pair.depths.clone().map(|texture| EyeDepth12 { texture, state: D3D12_RESOURCE_STATE_COMMON, rect: pair.depth_rect }), params })
}

/// Frame generation's real pair, at its time.
pub(super) fn send_due(due: Due) {
    let common = D3D12_RESOURCE_STATE_COMMON;
    send([(&due.images[0], common), (&due.images[1], common)], Some([due.pose; 2]), due.depth.as_ref());
    Counters::add(&COUNTERS.real);
}

/// The alternate-eye loop's step while [`enabled`] ([`present_step`]): the frame about to present
/// (`chain`'s back buffer) goes out as its pair, flat (a menu), or to both eyes (unlabelled). True:
/// the loop publishes nothing itself.
pub(super) fn before_present(driver: &Driver, chain: &IDXGISwapChain) -> bool {
    let Some(game) = *GAME.locked() else { return true };
    if let Ok(swap) = chain.cast::<IDXGISwapChain3>()
        && let Some(queue) = crate::queue12::game_queue(Some(&swap))
        // SAFETY: COM calls on the game's live swapchain, on the thread that presents it.
        && let Ok(buffer) = unsafe { swap.GetBuffer::<ID3D12Resource>(swap.GetCurrentBackBufferIndex()) }
    {
        let ready = {
            let mut publisher = PUBLISHER.locked();
            CHANNEL.locked().clone().is_some_and(|channel| publisher.get_or_insert_with(|| PairPublisher12::new(channel)).ready_for(&buffer, &queue))
        };
        if ready {
            let frame = Frame { image: (&buffer, D3D12_RESOURCE_STATE_PRESENT), present: driver.presents(), flat: driver.flat(), label: driver.take_label().map(|label| Pending { eye: label.eye, pose: label.pose }) };
            publish(&queue, frame, game);
        }
    }
    true
}

/// The same-frame loop's present of a frame fed to frame warping (one eye per frame; while
/// [`on_same_frame`]): `image` (in `state`: the eye as the loop would publish it, the HUD laid
/// over) of the frame labelled `label` goes out as its pair through the loop's pair sink, or to both
/// eyes (unlabelled). Whether a pair went out.
pub fn pair_present(queue: &ID3D12CommandQueue, image: &ID3D12Resource, state: D3D12_RESOURCE_STATES, label: Option<Pending>) -> bool {
    let Some(game) = *GAME.locked() else { return false };
    let frame = Frame { image: (image, state), present: crate::same_frame::PAIRS.presents(), flat: None, label };
    publish(queue, frame, game) == Step::Published
}

/// A frame about to present: the image that goes out as its eye (in its state), the present showing
/// it, why it goes flat, its label.
pub(super) struct Frame<'a> {
    pub(super) image: (&'a ID3D12Resource, D3D12_RESOURCE_STATES),
    pub(super) present: u64,
    pub(super) flat: Option<crate::driver::Flat>,
    pub(super) label: Option<Pending>,
}

/// The frame about to present (presented on the game's `queue`): its pair published, or the frame
/// flat, or both eyes of an unlabelled frame.
pub(super) fn publish(queue: &ID3D12CommandQueue, frame: Frame, game: GameFns) -> Step {
    let Frame { image: presented, present, flat, label } = frame;
    let mut guard = STATE.locked();
    let Some(state) = state_for(&mut guard, queue) else { return Step::Failed };
    let prepared = state.prepared.take().filter(|p| p.present == present && label.is_some_and(|l| l.eye.min(1) == p.eye));
    let common = D3D12_RESOURCE_STATE_COMMON;
    let now = Instant::now();
    let out = state.presents.out(flat.is_some(), label.is_some(), prepared.is_some(), now);
    if out.forgets() {
        forget(state);
    }
    let prepared = match (out, label, prepared) {
        (Out::Flat, ..) => {
            drop(guard);
            Counters::add(&COUNTERS.flat);
            return PACING.now_then(|| send([presented; 2], None, None), None);
        }
        // The pause screen and loading (the player camera is not updated) go to both eyes with the
        // newest pose.
        (Out::BothEyes, ..) => {
            drop(guard);
            let pose = game.head().current().unwrap_or_default();
            return PACING.now_then(|| send([presented; 2], Some([pose; 2]), None), None);
        }
        (Out::Plain, Some(label), _) => return plain(state, queue, presented, label.eye.min(1), label.pose, present),
        (Out::Warped, _, Some(prepared)) => prepared,
        // A lone unlabelled frame in play is skipped (a flat frame would flash).
        _ => return Step::Waiting,
    };
    let Prepared { eye, pose, pair, shown, generated, .. } = prepared;
    let depth = pair_depth(&pair, game.units_per_metre);
    let Some(shown) = shown else {
        // AFW alone: the frame presented (the HUD laid over) and the warped eye.
        drop(guard);
        let images = in_eye_order(eye, presented, (&pair.images[1 - eye], common));
        return PACING.now_then(|| send(images, Some([pose; 2]), depth.as_ref()), None);
    };
    // Frame generation: the generated pair now, the real pair half a frame later.
    let due = state.interval.due(now);
    let outputs: Option<[ID3D12Resource; 2]> = generated.and_then(|_| Some([state.streams[0].as_ref()?.output.clone(), state.streams[1].as_ref()?.output.clone()]));
    drop(guard);
    let real = Due { images: shown, pose, depth };
    match (generated, outputs) {
        (Some(halfway), Some(outputs)) => {
            Counters::add(&COUNTERS.generated);
            // Halfway between the last pair and this one: this (the newer) pair's depth.
            let depth = real.depth.clone();
            PACING.now_then(|| send([(&outputs[0], common), (&outputs[1], common)], Some([halfway; 2]), depth.as_ref()), Some((due, real)))
        }
        _ => PACING.now_then(
            || {
                Counters::add(&COUNTERS.real);
                send([(&real.images[0], common), (&real.images[1], common)], Some([real.pose; 2]), real.depth.as_ref())
            },
            None,
        ),
    }
}

/// A frame that could not be warped (`presented`, in its state): kept as its eye's newest, and
/// published beside the other eye's newest a frame apart (plain alternate eyes), when that one is
/// the frame just before.
pub(super) fn plain(state: &mut State, queue: &ID3D12CommandQueue, presented: (&ID3D12Resource, D3D12_RESOURCE_STATES), eye: usize, pose: HeadPose, present: u64) -> Step {
    let Ok(device) = d3d12::device_of(queue) else { return Step::Failed };
    let (buffer, buffer_state) = presented;
    // SAFETY: reads a descriptor.
    let desc = unsafe { buffer.GetDesc() };
    let mut slot = state.plain[eye].take();
    state.presents.lose_plain(eye);
    let Some(copy) = d3d12::fitting(&mut slot, &device, desc.Width, desc.Height, desc.Format, D3D12_RESOURCE_FLAG_NONE) else { return Step::Failed };
    let Ok(Some(list)) = state.recorder.begin() else { return Step::Waiting };
    d3d12::copy_whole(&list, &copy, buffer, buffer_state);
    if state.recorder.submit(queue).is_err() {
        return Step::Failed;
    }
    state.plain[eye] = Some(copy);
    let other = state.presents.plain(eye, pose, present).and_then(|pose| Some((state.plain[1 - eye].clone()?, pose)));
    let Some((other, other_pose)) = other else { return Step::Waiting };
    Counters::add(&COUNTERS.plain);
    let images = in_eye_order(eye, presented, (&other, D3D12_RESOURCE_STATE_COMMON));
    let poses = in_eye_order(eye, pose, other_pose);
    PACING.now_then(|| send(images, Some(poses), None), None)
}

/// Both eyes' history dropped (a menu, a cut): the next pair starts afresh.
pub(super) fn forget(state: &mut State) {
    state.afw.forget();
    state.presents.forget();
    state.streams.iter_mut().flatten().for_each(|s| s.primed = false);
}

/// Stops the pacing thread and frees everything once the GPU is done with it.
pub fn close() {
    ON.store(false, Release);
    DRIVER.choose_eyes(None);
    crate::driver::leave_present(present_step);
    crate::providers::leave_hud(warp, lay_hud);
    PACING.stop();
    if let Some(state) = STATE.locked().take() {
        if !state.afw.idle(5000) {
            log!("frame warping: GPU still busy at stop; its resources are left allocated");
            std::mem::forget(state);
        } else {
            monaka_framegen::free_when_idle(state, |s| &s.recorder);
        }
    }
    if let Some(mut publisher) = PUBLISHER.locked().take() {
        log!("frame warping: {} pairs published, {} failures", publisher.pairs(), publisher.failures());
        publisher.close();
    }
    inputs::clear();
}

pub(super) fn log_report(_: f64) {
    log!("{}", report());
}

/// The periodic report's line (the core's, [`afw::report_line`]).
pub fn report() -> String {
    let warped = STATE.lock().ok().and_then(|s| s.as_ref().map(|s| s.afw.counts())).unwrap_or_default();
    let with_depth = PUBLISHER.lock().ok().and_then(|p| p.as_ref().map(|p| p.pairs_with_depth())).unwrap_or(0);
    let framegen = framegen_running().then(monaka_framegen::eyes_skipped);
    afw::report_line(inputs::copies(), warped, &COUNTERS, with_depth, framegen)
}

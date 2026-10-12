//! Whether a feature's component is on, against the games' own question about their component.

use super::*;

struct Toy;

impl Game for Toy {
    const MANIFESTS: &'static [Manifest] = &[];
    const ADAPTER: Adapter<Toy> = Adapter { components: &[&EYES, &HAND_RIG, &FINGERS] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> {
        Ok(())
    }
}

const EYES: Component<Toy> = Component { name: "eyes", features: &[Feature::Stereo], ..Component::NONE };
const HAND_RIG: Component<Toy> = Component { name: "hand rig", features: &[Feature::HandRig], ..Component::NONE };
const FINGERS: Component<Toy> = Component { name: "fingers", features: &[Feature::FingerRig], ..Component::NONE };

/// The hand rig is on as Dishonored asked it of its own component's place; a game with no hand rig
/// (Control) said no.
#[test]
fn the_hand_rig_is_on_when_its_component_is() {
    let at = place::<Toy>(&HAND_RIG);
    for on in 0..8u64 {
        assert_eq!(provided_by_on(Toy::ADAPTER.components, on, Feature::HandRig), on & (1 << at) != 0, "{on:03b}");
        assert!(!provided_by_on(&[&EYES, &FINGERS], on, Feature::HandRig));
    }
}

//! How a game renders in VR, one scheme for every game: the stereo method ([`StereoMethod`], the
//! `stereo` option) and the frame generator ([`FrameGenerator`], the `framegen` option), each game
//! offering only the methods and generators it has ([`stereo_method`], [`frame_generation`]); and
//! the order of the Rendering tab ([`place`]).
//!
//! Which frame generation runs follows the method: per eye with Standard (Same-frame) and AER
//! (Alt-eye), pairs between the warped pairs with AFW (Frame-warp), none with Depth-stereo.
//!
//! A game lists what it offers by the enums, plainly (`StereoMethod::Afw.offer()`) or marked
//! experimental (`StereoMethod::Afw.experimental()`): still offered, its label saying so and the
//! option's help saying why it is not the game's default yet. The default is typed, and one the
//! game does not offer stops the build.

use monaka_contract::manifest::{Kind, Opt, Shown, opt};
use crate::options::Options;

/// One offered value as the option lists it: (value, label).
pub type Offer = (&'static str, &'static str);

/// What a label ends with when the game marks its choice experimental.
const EXPERIMENTAL: &str = ", experimental";

/// How the two eyes are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StereoMethod {
    /// Both eyes rendered in one game frame.
    SameFrame,
    /// One eye per frame, the eyes taking turns.
    Aer,
    /// AER with each frame warped into the other eye with its depth.
    Afw,
    /// One centre render, both eyes made from its depth.
    DepthStereo,
}

const METHODS: [StereoMethod; 4] = [StereoMethod::SameFrame, StereoMethod::Aer, StereoMethod::Afw, StereoMethod::DepthStereo];

/// Stereo off, on the command line only (the last of a game's list, `stereo=off`): both eyes the
/// game's own camera, or (The Beast) the probes alone.
pub const STEREO_OFF: Offer = ("off", "Off");

const STEREO_HELP: &str = "Standard (Same-frame) renders both eyes in one game frame. AER (Alt-eye) renders one eye per frame, taking turns. AFW (Frame-warp) is AER with each frame also warped into the other eye with its depth. Depth-stereo (synthetic) renders once from the centre and makes both eyes from its depth.";
const STEREO_HELP_EXPERIMENTAL: &str = "Standard (Same-frame) renders both eyes in one game frame. AER (Alt-eye) renders one eye per frame, taking turns. AFW (Frame-warp) is AER with each frame also warped into the other eye with its depth. Depth-stereo (synthetic) renders once from the centre and makes both eyes from its depth. A method marked experimental is not yet this game's default because it is still being proven.";

impl StereoMethod {
    /// Its value, label and label when marked experimental.
    const fn names(self) -> (&'static str, &'static str, &'static str) {
        match self {
            StereoMethod::SameFrame => ("same", "Standard (Same-frame)", "Standard (Same-frame), experimental"),
            StereoMethod::Aer => ("aer", "AER (Alt-eye)", "AER (Alt-eye), experimental"),
            StereoMethod::Afw => ("afw", "AFW (Frame-warp)", "AFW (Frame-warp), experimental"),
            StereoMethod::DepthStereo => ("depth", "Depth-stereo (synthetic)", "Depth-stereo (synthetic), experimental"),
        }
    }

    /// Its name as the window shows it.
    pub const fn label(self) -> &'static str {
        self.names().1
    }

    /// The method as a game offers it.
    pub const fn offer(self) -> Offer {
        let (value, label, _) = self.names();
        (value, label)
    }

    /// The method offered, marked experimental.
    pub const fn experimental(self) -> Offer {
        let (value, _, label) = self.names();
        (value, label)
    }

    /// Its value on the command line and in the options file.
    pub const fn value(self) -> &'static str {
        self.names().0
    }

    /// The method the run's `stereo` option names among those `opt` (the game's [`stereo_method`])
    /// offers, else the game's default.
    pub fn chosen(opt: &Opt, options: &Options) -> StereoMethod {
        let (offered, default) = offered(opt);
        let read = |text: &str| METHODS.into_iter().find(|method| method.value().eq_ignore_ascii_case(text.trim()) && offered.iter().any(|(id, _)| *id == method.value()));
        options.text(opt.id).and_then(read).or_else(|| read(default)).unwrap_or(StereoMethod::SameFrame)
    }
}

/// The stereo method option, offering `methods` (built by [`StereoMethod::offer`] and
/// [`StereoMethod::experimental`], ending with [`STEREO_OFF`], which only the command line
/// offers), `default` chosen unless the player picks another. A default not offered in the window
/// fails the build.
pub const fn stereo_method(methods: &'static [Offer], default: StereoMethod) -> Opt {
    let window = if !methods.is_empty() && same(methods[methods.len() - 1].0, STEREO_OFF.0) { methods.len() - 1 } else { methods.len() };
    assert!(offers(methods, window, default.value()), "the stereo method's default is not offered");
    let help = if any_experimental(methods, window) { STEREO_HELP_EXPERIMENTAL } else { STEREO_HELP };
    opt("stereo", "Stereo method", help, Kind::Choice { choices: methods, default: default.value(), window }).on(Shown::Rendering)
}

/// What makes the generated frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameGenerator {
    Off,
    /// AMD's FSR frame generation.
    Fsr,
}

const GENERATORS: [FrameGenerator; 2] = [FrameGenerator::Off, FrameGenerator::Fsr];

const FRAMEGEN_HELP: &str = "Adds generated frames between the rendered ones: each eye's own with Standard (Same-frame) and AER (Alt-eye), in-between pairs with AFW (Frame-warp). Not available with Depth-stereo (synthetic).";
const FRAMEGEN_HELP_EXPERIMENTAL: &str = "Adds generated frames between the rendered ones: each eye's own with Standard (Same-frame) and AER (Alt-eye), in-between pairs with AFW (Frame-warp). Not available with Depth-stereo (synthetic). A generator marked experimental is not yet this game's default because it is still being proven.";

impl FrameGenerator {
    /// Its value, label and label when marked experimental.
    const fn names(self) -> (&'static str, &'static str, &'static str) {
        match self {
            FrameGenerator::Off => ("off", "Off", "Off, experimental"),
            FrameGenerator::Fsr => ("fsr", "FSR frame generation", "FSR frame generation, experimental"),
        }
    }

    /// The generator as a game offers it.
    pub const fn offer(self) -> Offer {
        let (value, label, _) = self.names();
        (value, label)
    }

    /// The generator offered, marked experimental.
    pub const fn experimental(self) -> Offer {
        let (value, _, label) = self.names();
        (value, label)
    }

    /// Its value on the command line and in the options file.
    pub const fn value(self) -> &'static str {
        self.names().0
    }

    /// The generator the run's `framegen` option names among those `opt` (the game's
    /// [`frame_generation`]) offers, else the game's default.
    pub fn chosen(opt: &Opt, options: &Options) -> FrameGenerator {
        let (offered, default) = offered(opt);
        let read = |text: &str| GENERATORS.into_iter().find(|generator| generator.value().eq_ignore_ascii_case(text.trim()) && offered.iter().any(|(id, _)| *id == generator.value()));
        options.text(opt.id).and_then(read).or_else(|| read(default)).unwrap_or(FrameGenerator::Off)
    }
}

/// The frame generation option, offering `generators` (built by [`FrameGenerator::offer`] and
/// [`FrameGenerator::experimental`]), `default` chosen unless the player picks another. A default
/// not offered fails the build.
pub const fn frame_generation(generators: &'static [Offer], default: FrameGenerator) -> Opt {
    assert!(offers(generators, generators.len(), default.value()), "the frame generator's default is not offered");
    let help = if any_experimental(generators, generators.len()) { FRAMEGEN_HELP_EXPERIMENTAL } else { FRAMEGEN_HELP };
    opt("framegen", "Frame generation", help, Kind::Choice { choices: generators, default: default.value(), window: generators.len() }).on(Shown::Rendering)
}

/// The values the window offers in choice option `opt`, and its default.
fn offered(opt: &Opt) -> (&'static [Offer], &'static str) {
    match opt.kind {
        Kind::Choice { choices, default, window } => (&choices[..window.min(choices.len())], default),
        _ => (&[], ""),
    }
}

/// Whether the first `window` of `offered` hold `value`.
const fn offers(offered: &[Offer], window: usize, value: &str) -> bool {
    let mut i = 0;
    while i < window {
        if same(offered[i].0, value) {
            return true;
        }
        i += 1;
    }
    false
}

/// Whether any of the first `window` of `offered` is marked experimental.
const fn any_experimental(offered: &[Offer], window: usize) -> bool {
    let mut i = 0;
    while i < window {
        if ends_with(offered[i].1, EXPERIMENTAL) {
            return true;
        }
        i += 1;
    }
    false
}

const fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && ends_with(a, b)
}

const fn ends_with(text: &str, end: &str) -> bool {
    let (text, end) = (text.as_bytes(), end.as_bytes());
    if end.len() > text.len() {
        return false;
    }
    let start = text.len() - end.len();
    let mut i = 0;
    while i < end.len() {
        if text[start + i] != end[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The Rendering tab's options in their order: the stereo method, frame generation, the upscaler per
/// eye, depth to the headset, foveation, the render size.
const ORDER: [&str; 9] = ["stereo", "framegen", "upscale_per_eye", "fsr", "fsr_scale", "fsr_sharpness", "headset_depth", "foveated_rendering", "render_size"];

/// Where option `opt` goes among its tab's (the window sorts each tab's options by it, keeping the
/// game's order otherwise): the Rendering tab in [`ORDER`], a game's own after them.
pub fn place(opt: &Opt) -> usize {
    match opt.shown {
        Shown::Rendering => ORDER.iter().position(|id| *id == opt.id).unwrap_or(ORDER.len()),
        _ => 0,
    }
}

/// The run's Rendering-tab choices among `options` (a game's) as `options` say them, one
/// `Label: value` a line in the tab's order: the Stats panel's settings. An option that means
/// nothing with the others' values (its `when` does not hold) is left out.
pub fn summary(opts: &[Opt], options: &Options) -> String {
    let value = |opt: &Opt| options.text(opt.id).map(|v| v.trim().to_owned()).unwrap_or_else(|| opt.default_text());
    let applies = |opt: &Opt| opt.when.is_none_or(|(id, wanted)| opts.iter().find(|o| o.id == id).is_some_and(|o| monaka_contract::manifest::matches(&value(o), wanted)));
    let mut shown: Vec<&Opt> = opts.iter().filter(|o| o.shown == Shown::Rendering && o.producer && applies(o)).collect();
    shown.sort_by_key(|o| place(o));
    let lines: Vec<String> = shown
        .into_iter()
        .filter_map(|opt| {
            let raw = value(opt);
            let text = match opt.kind {
                Kind::Choice { choices, default, .. } => {
                    let chosen = choices.iter().find(|(id, _)| id.eq_ignore_ascii_case(&raw)).or_else(|| choices.iter().find(|(id, _)| *id == default));
                    chosen.map(|(_, label)| (*label).to_owned())?
                }
                Kind::Switch { default } => (if monaka_contract::manifest::parse_switch(&raw).unwrap_or(default) { "On" } else { "Off" }).to_owned(),
                Kind::Number { .. } => raw,
                Kind::Text | Kind::Size if raw.is_empty() => return None,
                Kind::Text | Kind::Size => raw,
            };
            Some(format!("{}: {text}", opt.label))
        })
        .collect();
    lines.join("\n")
}

#[cfg(test)]
mod tests;

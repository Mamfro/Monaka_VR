//! Shooting from the barrel, as a ready-made (`gun`): a gun's shots leave its muzzle along its
//! barrel, where the gun in the hand points, instead of from the eye along the game's aim. Each
//! pellet keeps its spread around the aim, turned onto the barrel.
//!
//! A game implements the fire hook (`crate::launch::Fires`: each projectile of the player's shot
//! asked of the ready-made, with the aim the game made it along and, with a gun in each hand, the
//! hand that fired it) and the held item (the gun in that hand, its barrel's line on its holder,
//! `crate::held`). The barrel is that line where the gun is ([`crate::held::in_world`]: where the
//! hand rig last posed the holder, or where the game put the gun itself), the muzzle at its end.

use monaka_core::feature::Feature;
use crate::held::{self, HeldItems};
use crate::launch::{self, Fired, Fires, Shot, Weapon};
use crate::held::Kind;
use monaka_core::math::{self, Vec3};
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Lack, Need, On, Run, Start, Switch};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// The holder must have been posed this recently (milliseconds): a pose the rig has not made
/// lately (the arms left to the game) is not where the gun is.
const POSED_FRESH: u64 = 100;

/// Shooting from the barrel for game `G`, with motion controls (`shoot_from_barrel=0`: shots from
/// the eye along the aim, for a test). A game says what it works on top of by struct update.
pub const fn gun<G: Fires + HeldItems>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::HandRig)],
        name: "gun",
        switch: Some(Switch::new("shoot_from_barrel", true, "shots from the eye")),
        on: on::<G>,
        install: install::<G>,
        release,
        ..Component::NONE
    }
}

fn on<G: Fires + HeldItems>(run: &Run<'_, G>) -> On {
    On::when(run.shared.aim.rig, "aim is not motion controls")
}

/// The aiming hand, and the shots placed.
static AIM_SIDE: Mutex<usize> = Mutex::new(monaka_contract::protocol::RIGHT_HAND);
static SHOTS: AtomicU64 = AtomicU64::new(0);
static INSTALLED: Mutex<bool> = Mutex::new(false);

fn install<G: Fires + HeldItems>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    <G as Fires>::install(start)?;
    *AIM_SIDE.locked() = start.shared.aim.side;
    launch::aim_shots(Weapon::Gun, Some(shot::<G>));
    *INSTALLED.locked() = true;
    log!("shooting from the barrel: a gun's shots leave its muzzle along its barrel");
    Ok(())
}

fn release() {
    launch::aim_shots(Weapon::Gun, None);
    if std::mem::take(&mut *INSTALLED.locked()) {
        log!("shooting from the barrel: {} projectiles placed", SHOTS.load(Relaxed));
    }
}

/// Where the projectile `fired` leaves from and which way, for a barrel from `muzzle` along
/// `barrel` (unit): its turn away from the game's aim put on the barrel. None without the game's aim
/// or a velocity to turn.
pub fn from_barrel(fired: &Fired, muzzle: Vec3, barrel: Vec3) -> Option<Shot> {
    let aim = fired.aim?;
    let along = math::unit(fired.game.velocity)?;
    Some(Shot { from: muzzle, direction: math::turned(barrel, aim, along), speed: None })
}

fn shot<G: Fires + HeldItems>(fired: &Fired) -> Option<Shot> {
    let side = fired.side.unwrap_or_else(|| *AIM_SIDE.locked());
    if held::now::<G>(side).kind != Kind::Gun {
        return None;
    }
    let posed = held::in_world::<G>(side).filter(|posed| monaka_channel::tick().saturating_sub(posed.posed) < POSED_FRESH)?;
    let line = posed.line?;
    let shot = from_barrel(fired, line.end().unwrap_or(line.from), line.along)?;
    let n = SHOTS.fetch_add(1, Relaxed) + 1;
    if n <= 20 {
        log!("shooting from the barrel: projectile {n} from {:.2?} along {:.2?}", shot.from, shot.direction);
    }
    Some(shot)
}

#[cfg(test)]
mod tests;

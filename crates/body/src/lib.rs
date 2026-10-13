//! The player's body in VR, first of all its arms on the controllers. A game provides its skeleton ([`Skeleton`]: its
//! elements, their poses now, a way to write them back) and the moment its pose is final; the
//! ready-made hand rig ([`hand_rig`]) does everything between: the arms onto the controllers, the
//! fingers on finger tracking ([`finger_rig`]), one write of the bones that changed. For a skeleton
//! of named elements (Dying Light 1 and 2: `r_hand`, `l_upperarm`, `r_handholder`, ...) the arms
//! go on by [`follow`](mod@follow): the moment the game has placed the arms against the player
//! camera, a [`Snapshot`] of the animated arms ([`skeleton`]) is taken before the game's own
//! adjustments, and they are moved after them.
//!
//! The aim-side wrist goes into its controller's palm (`HandPose::palm`: the grip pose's position,
//! the aim pose's direction), with the fingers and what they hold animated against it as the game
//! does; a gun instead sits in the palm by its grip, pointing where the controller aims. Each upper
//! arm and forearm reaches its hand from a shoulder under the player's real head
//! ([`monaka_core::reach`]). The off hand rides a weapon the game holds two-handed, else follows its
//! own controller. The rest of the body goes with the head, as animated (moving the whole skeleton
//! with the hand would move the body with it). The look methods steer the character where the head
//! (or a hand) looks ([`look`]), and
//! without head aim the [`pitch_lock`] holds vertical look to level. [`melee`] turns swings into
//! attacks. Climbing and hanging, the arms go back to the game's animation ([`traverse`]).
//! [`roomscale`] walks the character after the player's real steps. [`hand_trail`] keeps the
//! hands' recent way in the world. [`held`] is what each hand holds, as the game provides it, and
//! [`launch`] what leaves the hands (a shot fired, an item thrown). Ready-mades riding on them:
//! [`reload`] (the hand to the gun and away), [`duck`] (crouching by lowering the head), [`lockpick`]
//! (the pick and its tool turned by twisting the hands), [`throw`] (what the hands throw leaving
//! as they threw it), [`bow`] (the string drawn by the hand, the arrow loosed along the bow) and
//! [`gun`] (shots from the barrel). The hand rig lines a gun held pointing elsewhere up with its
//! controller ([`gun_hold`]).

pub mod bow;
pub mod controllers;
pub mod duck;
pub mod feature;
pub mod finger_rig;
pub mod fingers;
pub mod follow;
pub mod gun;
pub mod gun_hold;
pub mod hand_rig;
pub mod hand_trail;
pub mod hands;
pub mod held;
pub mod launch;
pub mod lockpick;
pub mod look;
pub mod melee;
pub mod melee_hit;
pub mod pitch_lock;
pub mod reload;
pub mod roomscale;
pub mod skeleton;
pub mod throw;
pub mod traverse;

// The skeleton provider and the arms put on the controllers, by the names the games use.
pub use follow::{BoneAxes, MELEE_BLADE, MELEE_GRIP, Placed, Pose, declare_mirror, declare_one_handed, follow, hands_apart, off_hand_on_weapon, report};
pub(crate) use follow::{carry, remember};
pub use skeleton::{Frames, Skeleton, Snapshot};


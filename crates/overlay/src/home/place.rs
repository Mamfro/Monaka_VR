//! Where the home's panels float and where a controller points on them. The panels stand on a
//! ring round where the head was when the home opened, each turned to face it, flat (not curved):
//! the one in front straight ahead of where the head faced, the other two to its sides. Turning the
//! ring (LB, RB) brings another panel in front, so none needs the head turned far to use.

use monaka_core::math::{self, Quat, Vec3};

/// How far the panels stand from the head, and how far below the eyes (metres).
pub const RADIUS: f32 = 1.2;
pub const BELOW_EYES: f32 = 0.15;
/// How far round the ring a side panel stands from the one in front (radians): clear of it at
/// [`WIDTH`], close enough to read at a glance.
pub const SIDE_ANGLE: f32 = 50.0 * std::f32::consts::PI / 180.0;
/// A panel's width (metres) and its image (pixels); its height follows from the image.
pub const WIDTH: f32 = 0.9;
pub const PIXELS: [u32; 2] = [1024, 768];

/// A panel's height (metres).
pub fn height() -> f32 {
    WIDTH * PIXELS[1] as f32 / PIXELS[0] as f32
}

/// Where the ring was opened: the head's position and the way it faced (yaw only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub eye: Vec3,
    pub yaw: f32,
}

impl Ring {
    /// The ring for a head at `position` facing `orientation` (its pitch and roll left out).
    pub fn at(orientation: Quat, position: Vec3) -> Self {
        Self { eye: position, yaw: math::yaw(orientation) }
    }

    /// Where panel `page` (of `pages`) floats with panel `front` in front: its orientation and
    /// centre. The pages run left to right round the ring.
    pub fn place(&self, page: usize, front: usize, pages: usize) -> (Quat, Vec3) {
        self.at_angle(slot_angle(page, front, pages))
    }

    /// Where a panel floats `angle` radians round the ring to the right of straight ahead.
    pub fn at_angle(&self, angle: f32) -> (Quat, Vec3) {
        let orientation = math::yaw_rotation(self.yaw - angle);
        let out = math::rotate(orientation, [0.0, 0.0, -RADIUS]);
        (orientation, [self.eye[0] + out[0], self.eye[1] - BELOW_EYES, self.eye[2] + out[2]])
    }
}

/// How far round the ring to the right of straight ahead panel `page` (of `pages`) stands with
/// panel `front` in front (radians): 0 in front, a [`SIDE_ANGLE`] to either side (more pages wrap
/// further round).
pub fn slot_angle(page: usize, front: usize, pages: usize) -> f32 {
    let ahead = (page + pages - front) % pages;
    let slot = if ahead * 2 > pages { ahead as f32 - pages as f32 } else { ahead as f32 };
    slot * SIDE_ANGLE
}

/// How long the ring takes to turn (seconds).
pub const TURN_SECONDS: f32 = 0.3;

/// A turn of the ring: each panel going round from where it was to its new place, all in the
/// direction of the turn, so one that leaves one end goes all the way round to the other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turning {
    from: [f32; 3],
    to: [f32; 3],
}

impl Turning {
    /// From the panels' angles now (`from`) to their places with `front` in front, turning
    /// `steps` (positive: the ring turns left, bringing the panel on the right in front).
    pub fn new(from: [f32; 3], front: usize, steps: isize) -> Self {
        let full = std::f32::consts::TAU;
        let to = std::array::from_fn(|page| {
            let mut delta = slot_angle(page, front, 3) - from[page];
            // Every panel moves the way the ring turns; a panel that would move back goes round.
            if steps > 0 && delta > 1e-4 {
                delta -= full;
            } else if steps < 0 && delta < -1e-4 {
                delta += full;
            }
            from[page] + delta
        });
        Self { from, to }
    }

    /// The panels' angles `progress` of the way through (0 to 1, eased at both ends).
    pub fn at(&self, progress: f32) -> [f32; 3] {
        let t = progress.clamp(0.0, 1.0);
        let eased = t * t * (3.0 - 2.0 * t);
        std::array::from_fn(|page| self.from[page] + (self.to[page] - self.from[page]) * eased)
    }
}

/// Where a ray from `origin` along `direction` meets a panel of `size` (metres) at `centre`,
/// turned `orientation` (its face toward +Z, as a quad layer's): how far along the ray, and where
/// on the panel from its top left, 0 to 1 across and down. `None`: it misses, comes from behind,
/// or points away.
pub fn hit(orientation: Quat, centre: Vec3, size: [f32; 2], origin: Vec3, direction: Vec3) -> Option<(f32, [f32; 2])> {
    let normal = math::rotate(orientation, [0.0, 0.0, 1.0]);
    let toward = math::dot(direction, normal);
    if toward > -1e-6 {
        return None;
    }
    let along = math::dot(math::sub(centre, origin), normal) / toward;
    if along <= 0.0 {
        return None;
    }
    let point = math::add(origin, math::scale(direction, along));
    let local = math::rotate(math::conjugate(orientation), math::sub(point, centre));
    let (across, down) = (local[0] / size[0] + 0.5, 0.5 - local[1] / size[1]);
    ((0.0..=1.0).contains(&across) && (0.0..=1.0).contains(&down)).then_some((along, [across, down]))
}

/// The way a controller's aim pose points (its forward, -Z).
pub fn pointing(orientation: Quat) -> Vec3 {
    math::rotate(orientation, [0.0, 0.0, -1.0])
}

#[cfg(test)]
mod tests;

//! The camera of both view modes, CSS-pixel projection and the terrain ray cast of picking.
//!
//! World space: tile `(x, y)` covers `x..x + 1` on the X axis and `y..y + 1` on the Z axis,
//! Y points up and the ground (plains, swamps) is at `y = 0`.

use glam::camera::rh;
use glam::{Mat4, Vec2, Vec3};

use crate::ViewMode;

/// Tiles per room side, as a world length.
pub const ROOM: f32 = 50.0;
/// Vertical field of view of the perspective camera.
const FOV_Y: f32 = std::f32::consts::FRAC_PI_4;
/// Bounds of the visible half height (world units at the target).
const MIN_HALF_HEIGHT: f32 = 2.0;
const MAX_HALF_HEIGHT: f32 = 45.0;
/// Bounds of the orbit camera's elevation; the lower bound keeps every ray of the view
/// pointing down (it exceeds half the field of view).
const MIN_PITCH: f32 = 0.45;
const MAX_PITCH: f32 = 1.55;
/// Orbit angle per CSS pixel dragged.
const ORBIT_PER_PIXEL: f32 = 0.008;
/// Height of the top-down camera above the ground.
const TOP_DOWN_EYE: f32 = 100.0;

/// The camera shared by both modes: top-down is an orthographic camera looking straight down
/// at the target, perspective an orbit camera around it at a distance that frames the same
/// half height.
#[derive(Clone, Debug)]
pub struct Camera {
    /// The projection in use.
    pub mode: ViewMode,
    /// Ground point the camera looks at.
    target: Vec3,
    /// Half of the visible height at the target, in tiles.
    half_height: f32,
    /// Orbit angle around the vertical axis; 0 looks north (towards smaller tile `y`).
    yaw: f32,
    /// Orbit elevation above the ground plane.
    pitch: f32,
    /// Canvas size in CSS pixels.
    size: Vec2,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            mode: ViewMode::TopDown,
            target: Vec3::new(ROOM / 2.0, 0.0, ROOM / 2.0),
            half_height: ROOM / 2.0 + 1.0,
            yaw: 0.0,
            pitch: 0.95,
            size: Vec2::ONE,
        }
    }
}

impl Camera {
    /// Sets the canvas size in CSS pixels (at least one pixel each way).
    pub fn resize(&mut self, width: f32, height: f32) {
        self.size = Vec2::new(width.max(1.0), height.max(1.0));
    }

    /// Frames the whole room (the view's initial framing).
    pub fn fit(&mut self) {
        self.target = Vec3::new(ROOM / 2.0, 0.0, ROOM / 2.0);
        self.half_height = (ROOM / 2.0 + 1.0) * (1.0 / self.aspect()).max(1.0);
        self.half_height = self.half_height.clamp(MIN_HALF_HEIGHT, MAX_HALF_HEIGHT);
    }

    fn aspect(&self) -> f32 {
        self.size.x / self.size.y
    }

    /// Distance of the orbit camera from its target.
    fn distance(&self) -> f32 {
        self.half_height / (FOV_Y / 2.0).tan()
    }

    /// The eye position in the current mode.
    fn eye(&self) -> Vec3 {
        match self.mode {
            ViewMode::TopDown => self.target + Vec3::Y * TOP_DOWN_EYE,
            ViewMode::Perspective => {
                let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
                let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
                let direction = Vec3::new(sin_yaw * cos_pitch, sin_pitch, cos_yaw * cos_pitch);
                self.target + direction * self.distance()
            }
        }
    }

    /// World to clip space (wgpu: depth `0..1`, Y up).
    pub fn view_projection(&self) -> Mat4 {
        let eye = self.eye();
        match self.mode {
            ViewMode::TopDown => {
                // Up on screen is north (smaller tile `y`, negative Z).
                let view = rh::view::look_at_mat4(eye, self.target, Vec3::NEG_Z);
                let half_width = self.half_height * self.aspect();
                let projection = rh::proj::directx::orthographic(
                    -half_width,
                    half_width,
                    -self.half_height,
                    self.half_height,
                    1.0,
                    TOP_DOWN_EYE * 2.0,
                );
                projection * view
            }
            ViewMode::Perspective => {
                let view = rh::view::look_at_mat4(eye, self.target, Vec3::Y);
                let far = self.distance() + ROOM * 2.0;
                let projection = rh::proj::directx::perspective(FOV_Y, self.aspect(), 0.1, far);
                projection * view
            }
        }
    }

    /// CSS pixel → normalized device coordinates.
    fn ndc(&self, point: Vec2) -> Vec2 {
        Vec2::new(
            point.x / self.size.x * 2.0 - 1.0,
            1.0 - point.y / self.size.y * 2.0,
        )
    }

    /// The ray through a CSS pixel: origin and unit direction. Built from the camera basis
    /// (not by unprojecting the matrix, which loses precision at a near plane of 0.1).
    pub fn ray(&self, point: Vec2) -> (Vec3, Vec3) {
        let ndc = self.ndc(point);
        let eye = self.eye();
        let (forward, up) = match self.mode {
            ViewMode::TopDown => (Vec3::NEG_Y, Vec3::NEG_Z),
            ViewMode::Perspective => ((self.target - eye).normalize_or(Vec3::NEG_Y), Vec3::Y),
        };
        let right = forward.cross(up).normalize_or(Vec3::X);
        let up = right.cross(forward);
        match self.mode {
            ViewMode::TopDown => {
                let offset = right * ndc.x * self.half_height * self.aspect()
                    + up * ndc.y * self.half_height;
                (eye + offset, forward)
            }
            ViewMode::Perspective => {
                let tangent = (FOV_Y / 2.0).tan();
                let direction =
                    forward + right * ndc.x * tangent * self.aspect() + up * ndc.y * tangent;
                (eye, direction.normalize_or(forward))
            }
        }
    }

    /// The CSS pixel a world point projects to (`None` behind the camera).
    #[cfg(test)]
    pub fn project(&self, world: Vec3) -> Option<Vec2> {
        let clip = self.view_projection() * world.extend(1.0);
        (clip.w > 0.0).then(|| {
            let ndc = Vec2::new(clip.x, clip.y) / clip.w;
            Vec2::new(ndc.x + 1.0, 1.0 - ndc.y) / 2.0 * self.size
        })
    }

    /// Where the ray through a CSS pixel meets the horizontal plane at `height`.
    pub fn ground_point(&self, point: Vec2, height: f32) -> Option<Vec3> {
        let (origin, direction) = self.ray(point);
        if direction.y.abs() < 1e-6 {
            return None;
        }
        let t = (height - origin.y) / direction.y;
        (t >= 0.0).then(|| origin + direction * t)
    }

    /// Moves the camera so that the ground point under `from` ends up under `to`.
    pub fn pan(&mut self, from: Vec2, to: Vec2) {
        if let (Some(from), Some(to)) = (self.ground_point(from, 0.0), self.ground_point(to, 0.0)) {
            self.move_target(from - to);
        }
    }

    /// Multiplies the visible half height by `factor`, keeping the ground point under
    /// `anchor` in place.
    pub fn zoom(&mut self, anchor: Vec2, factor: f32) {
        let before = self.ground_point(anchor, 0.0);
        self.half_height = (self.half_height * factor).clamp(MIN_HALF_HEIGHT, MAX_HALF_HEIGHT);
        if let (Some(before), Some(after)) = (before, self.ground_point(anchor, 0.0)) {
            self.move_target(before - after);
        }
    }

    /// Orbits the perspective camera by a drag of `delta` CSS pixels.
    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw -= delta.x * ORBIT_PER_PIXEL;
        self.pitch = (self.pitch + delta.y * ORBIT_PER_PIXEL).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Moves the target, keeping it over the room.
    fn move_target(&mut self, delta: Vec3) {
        let target = self.target + Vec3::new(delta.x, 0.0, delta.z);
        self.target = Vec3::new(target.x.clamp(0.0, ROOM), 0.0, target.z.clamp(0.0, ROOM));
    }
}

/// The first tile whose terrain column the ray hits, walking the tiles it crosses (2D DDA)
/// between the top of the highest column (`max_height`) and the ground.
pub fn cast(
    origin: Vec3,
    direction: Vec3,
    max_height: f32,
    height: impl Fn(u8, u8) -> f32,
) -> Option<(u8, u8)> {
    if direction.y >= -1e-6 {
        return None;
    }
    let t_start = ((max_height - origin.y) / direction.y).max(0.0);
    let t_end = (0.0 - origin.y) / direction.y;
    if t_end < t_start {
        return None;
    }
    let start = origin + direction * t_start;
    let mut cell = (start.x.floor(), start.z.floor());
    let step = (direction.x.signum(), direction.z.signum());
    // Ray parameter of the next X and Z cell boundary, and between two boundaries.
    let boundary = |position: f32, cell: f32, direction: f32| {
        if direction.abs() < 1e-9 {
            f32::INFINITY
        } else {
            let next = if direction > 0.0 { cell + 1.0 } else { cell };
            t_start + (next - position) / direction
        }
    };
    let mut next = (
        boundary(start.x, cell.0, direction.x),
        boundary(start.z, cell.1, direction.z),
    );
    let delta = ((1.0 / direction.x).abs(), (1.0 / direction.z).abs());
    // A ray crosses at most 2 × 50 tile boundaries inside the room plus the approach.
    for _ in 0..512 {
        let exit = next.0.min(next.1).min(t_end);
        // The ray leaves the cell below its column's top, or reaches the ground in it.
        let hit = |tile: (u8, u8)| {
            exit >= t_end || origin.y + direction.y * exit <= height(tile.0, tile.1)
        };
        if let Some(tile) = tile(cell)
            && hit(tile)
        {
            return Some(tile);
        }
        if exit >= t_end {
            return None;
        }
        if next.0 < next.1 {
            cell.0 += step.0;
            next.0 += delta.0;
        } else {
            cell.1 += step.1;
            next.1 += delta.1;
        }
    }
    None
}

/// The tile of a cell, `None` outside the room.
fn tile(cell: (f32, f32)) -> Option<(u8, u8)> {
    let inside = |value: f32| (0.0..ROOM).contains(&value);
    if inside(cell.0) && inside(cell.1) {
        Some((tile_index(cell.0), tile_index(cell.1)))
    } else {
        None
    }
}

/// A whole cell coordinate in `0..50`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers pass whole numbers in 0..50"
)]
const fn tile_index(value: f32) -> u8 {
    value as u8
}

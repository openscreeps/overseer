//! The camera of both view modes, CSS-pixel projection and the terrain ray cast of picking.
//!
//! World space: the tile `(x, y)` of the room at [`RoomCoord`] `(rx, ry)` covers
//! `rx × 50 + x .. + 1` on the X axis and `ry × 50 + y .. + 1` on the Z axis ("world tiles"),
//! Y points up and the ground (plains, swamps) is at `y = 0`.

use glam::camera::rh;
use glam::{Mat4, Vec2, Vec3};
use overseer::map::{RoomCoord, RoomRect};

use crate::{CameraState, ViewMode};

/// Tiles per room side, as a world length.
pub const ROOM: f32 = 50.0;
/// Vertical field of view of the perspective camera.
const FOV_Y: f32 = std::f32::consts::FRAC_PI_4;
/// Smallest visible half height (world tiles at the target).
const MIN_HALF_HEIGHT: f32 = 2.0;
/// Largest visible half height, as a multiple of the half height that frames the bounds.
const MAX_FIT: f32 = 1.75;
/// Bounds of the orbit camera's elevation; the lower bound keeps every ray of the view
/// pointing down (it exceeds half the field of view).
const MIN_PITCH: f32 = 0.45;
const MAX_PITCH: f32 = 1.55;
/// Elevation of a new orbit camera.
const DEFAULT_PITCH: f32 = 0.95;
/// Orbit angle per CSS pixel dragged.
const ORBIT_PER_PIXEL: f32 = 0.008;
/// Height of the top-down camera above the ground.
const TOP_DOWN_EYE: f32 = 100.0;

/// The world position (X, Z) of a room's north-west corner.
#[expect(
    clippy::cast_precision_loss,
    reason = "room coordinates are far below 2^24 / 50"
)]
pub fn room_origin(room: RoomCoord) -> Vec2 {
    Vec2::new(room.x as f32, room.y as f32) * ROOM
}

/// The world position (X, Z) of a room's centre.
pub fn room_centre(room: RoomCoord) -> Vec2 {
    room_origin(room) + Vec2::splat(ROOM / 2.0)
}

/// The room of a world position.
#[expect(
    clippy::cast_possible_truncation,
    reason = "floored world positions of rooms fit i32"
)]
pub fn room_at(position: Vec2) -> RoomCoord {
    RoomCoord {
        x: (position.x / ROOM).floor() as i32,
        y: (position.y / ROOM).floor() as i32,
    }
}

/// The world-space extent (X, Z) of a rectangle of rooms.
pub fn rect_extent(rect: RoomRect) -> (Vec2, Vec2) {
    (
        room_origin(rect.min),
        room_origin(rect.max) + Vec2::splat(ROOM),
    )
}

/// The camera shared by both modes: top-down is an orthographic camera looking straight down
/// at the target, perspective an orbit camera around it at a distance that frames the same
/// half height. The target stays inside the bounds and the zoom between [`MIN_HALF_HEIGHT`]
/// and [`MAX_FIT`] times the framing of the bounds.
#[derive(Clone, Debug)]
pub struct Camera {
    /// The projection in use.
    pub mode: ViewMode,
    /// Ground point (X, Z) the camera looks at.
    target: Vec2,
    /// Half of the visible height at the target, in tiles.
    half_height: f32,
    /// Orbit angle around the vertical axis; 0 looks north (towards smaller tile `y`).
    yaw: f32,
    /// Orbit elevation above the ground plane.
    pitch: f32,
    /// Canvas size in CSS pixels.
    size: Vec2,
    /// World extent (X, Z) the target is kept in.
    bounds: (Vec2, Vec2),
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            mode: ViewMode::TopDown,
            target: Vec2::splat(ROOM / 2.0),
            half_height: ROOM / 2.0 + 1.0,
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            size: Vec2::ONE,
            bounds: (Vec2::ZERO, Vec2::splat(ROOM)),
        }
    }
}

impl Camera {
    /// Sets the canvas size in CSS pixels (at least one pixel each way).
    pub fn resize(&mut self, width: f32, height: f32) {
        self.size = Vec2::new(width.max(1.0), height.max(1.0));
        self.clamp();
    }

    /// Canvas size in CSS pixels.
    pub const fn size(&self) -> Vec2 {
        self.size
    }

    /// Keeps the target inside `rect` (and the zoom within its framing).
    pub fn set_bounds(&mut self, rect: RoomRect) {
        self.bounds = rect_extent(rect);
        self.clamp();
    }

    /// The half height that frames the whole bounds, with a small margin.
    fn fit_half_height(&self) -> f32 {
        let extent = self.bounds.1 - self.bounds.0;
        let margin = 1.0 + extent.max_element() * 0.01;
        (extent.y / 2.0 + margin)
            .max((extent.x / 2.0 + margin) / self.aspect())
            .max(MIN_HALF_HEIGHT)
    }

    fn max_half_height(&self) -> f32 {
        self.fit_half_height() * MAX_FIT
    }

    /// The half height that frames one room.
    pub fn room_half_height(&self) -> f32 {
        ((ROOM / 2.0 + 1.0) * (1.0 / self.aspect()).max(1.0))
            .clamp(MIN_HALF_HEIGHT, self.max_half_height())
    }

    /// Frames the whole bounds.
    pub fn fit(&mut self) {
        self.set_view(self.fit_view().0, self.fit_view().1);
    }

    /// Target and half height that frame the whole bounds.
    pub fn fit_view(&self) -> (Vec2, f32) {
        (
            (self.bounds.0 + self.bounds.1) / 2.0,
            self.fit_half_height(),
        )
    }

    /// The ground point looked at.
    pub const fn target(&self) -> Vec2 {
        self.target
    }

    /// Half of the visible height at the target, in tiles.
    pub const fn half_height(&self) -> f32 {
        self.half_height
    }

    /// Looks at `target` with half height `half_height` (both clamped).
    pub fn set_view(&mut self, target: Vec2, half_height: f32) {
        self.target = target;
        self.half_height = half_height;
        self.clamp();
    }

    /// The persistable state.
    pub const fn state(&self) -> CameraState {
        CameraState {
            centre: [self.target.x, self.target.y],
            half_height: self.half_height,
            mode: self.mode,
            yaw: self.yaw,
            pitch: self.pitch,
        }
    }

    /// Restores a state (clamped to the bounds and valid ranges).
    pub fn set_state(&mut self, state: CameraState) {
        let finite = |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
        self.mode = state.mode;
        self.target = Vec2::new(
            finite(state.centre[0], self.target.x),
            finite(state.centre[1], self.target.y),
        );
        self.half_height = finite(state.half_height, self.half_height);
        self.yaw = finite(state.yaw, 0.0).rem_euclid(std::f32::consts::TAU);
        self.pitch = finite(state.pitch, DEFAULT_PITCH);
        self.clamp();
    }

    /// Keeps target, zoom and pitch in range.
    fn clamp(&mut self) {
        self.target = self.target.clamp(self.bounds.0, self.bounds.1);
        self.half_height = self
            .half_height
            .clamp(MIN_HALF_HEIGHT, self.max_half_height().max(MIN_HALF_HEIGHT));
        self.pitch = self.pitch.clamp(MIN_PITCH, MAX_PITCH);
    }

    fn aspect(&self) -> f32 {
        self.size.x / self.size.y
    }

    /// CSS pixels per room side at the target.
    pub fn px_per_room(&self) -> f32 {
        self.size.y * ROOM / (2.0 * self.half_height)
    }

    /// World tiles per CSS pixel at the target.
    pub fn tiles_per_px(&self) -> f32 {
        2.0 * self.half_height / self.size.y
    }

    /// Distance of the orbit camera from its target.
    fn distance(&self) -> f32 {
        self.half_height / (FOV_Y / 2.0).tan()
    }

    fn target3(&self) -> Vec3 {
        Vec3::new(self.target.x, 0.0, self.target.y)
    }

    /// The eye position in the current mode.
    fn eye(&self) -> Vec3 {
        match self.mode {
            ViewMode::TopDown => self.target3() + Vec3::Y * TOP_DOWN_EYE,
            ViewMode::Perspective => {
                let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
                let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
                let direction = Vec3::new(sin_yaw * cos_pitch, sin_pitch, cos_yaw * cos_pitch);
                self.target3() + direction * self.distance()
            }
        }
    }

    /// World to clip space (wgpu: depth `0..1`, Y up).
    pub fn view_projection(&self) -> Mat4 {
        let eye = self.eye();
        match self.mode {
            ViewMode::TopDown => {
                // Up on screen is north (smaller tile `y`, negative Z).
                let view = rh::view::look_at_mat4(eye, self.target3(), Vec3::NEG_Z);
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
                let view = rh::view::look_at_mat4(eye, self.target3(), Vec3::Y);
                // The lowest ray of the view meets the ground within about 8 distances.
                let distance = self.distance();
                let (near, far) = ((distance * 0.01).max(0.05), distance * 12.0 + ROOM * 2.0);
                let projection = rh::proj::directx::perspective(FOV_Y, self.aspect(), near, far);
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
    /// (not by unprojecting the matrix, which loses precision at a small near plane).
    pub fn ray(&self, point: Vec2) -> (Vec3, Vec3) {
        let ndc = self.ndc(point);
        let eye = self.eye();
        let (forward, up) = match self.mode {
            ViewMode::TopDown => (Vec3::NEG_Y, Vec3::NEG_Z),
            ViewMode::Perspective => ((self.target3() - eye).normalize_or(Vec3::NEG_Y), Vec3::Y),
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

    /// The ground extent (X, Z) seen by the canvas: the bounding box of the ground points
    /// of its corners.
    pub fn visible_ground(&self) -> (Vec2, Vec2) {
        let corners = [
            Vec2::ZERO,
            Vec2::new(self.size.x, 0.0),
            Vec2::new(0.0, self.size.y),
            self.size,
        ];
        let (mut min, mut max) = (self.target, self.target);
        for corner in corners {
            if let Some(point) = self.ground_point(corner, 0.0) {
                let point = Vec2::new(point.x, point.z);
                min = min.min(point);
                max = max.max(point);
            }
        }
        (min, max)
    }

    /// Moves the camera so that the ground point under `from` ends up under `to`.
    pub fn pan(&mut self, from: Vec2, to: Vec2) {
        if let (Some(from), Some(to)) = (self.ground_point(from, 0.0), self.ground_point(to, 0.0)) {
            self.move_target(from - to);
        }
    }

    /// Multiplies the visible half height by `factor`, keeping the ground point under
    /// `anchor` in place (as far as the bounds allow).
    pub fn zoom(&mut self, anchor: Vec2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let before = self.ground_point(anchor, 0.0);
        self.half_height = (self.half_height * factor)
            .clamp(MIN_HALF_HEIGHT, self.max_half_height().max(MIN_HALF_HEIGHT));
        if let (Some(before), Some(after)) = (before, self.ground_point(anchor, 0.0)) {
            self.move_target(before - after);
        }
    }

    /// Orbits the perspective camera by a drag of `delta` CSS pixels.
    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw = (self.yaw - delta.x * ORBIT_PER_PIXEL).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + delta.y * ORBIT_PER_PIXEL).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Moves the target, keeping it inside the bounds.
    fn move_target(&mut self, delta: Vec3) {
        self.target =
            (self.target + Vec2::new(delta.x, delta.z)).clamp(self.bounds.0, self.bounds.1);
    }
}

/// The first world tile whose terrain column the ray hits, walking the tiles it crosses
/// (2D DDA) between the top of the highest column (`max_height`) and the ground. `height`
/// gives a tile's column height, `None` for tiles outside the pickable area (skipped; a ray
/// reaching the ground there hits nothing).
pub fn cast(
    origin: Vec3,
    direction: Vec3,
    max_height: f32,
    height: impl Fn(i32, i32) -> Option<f32>,
) -> Option<(i32, i32)> {
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
    // Every ray of the view descends at least 0.06 rad: from `max_height` it crosses a few
    // dozen tiles at most.
    for _ in 0..512 {
        let exit = next.0.min(next.1).min(t_end);
        let tile = whole(cell);
        // The ray leaves the cell below its column's top, or reaches the ground in it.
        if let Some(column) = height(tile.0, tile.1)
            && (exit >= t_end || origin.y + direction.y * exit <= column)
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

/// A floored cell coordinate as whole tiles.
#[expect(
    clippy::cast_possible_truncation,
    reason = "floored world tile coordinates fit i32"
)]
const fn whole(cell: (f32, f32)) -> (i32, i32) {
    (cell.0 as i32, cell.1 as i32)
}

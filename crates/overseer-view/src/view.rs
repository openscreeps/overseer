//! Everything of the viewer but the GPU: camera, scene and pointer gestures.

use glam::Vec2;

use crate::camera::{self, Camera};
use crate::mesh::WALL_HEIGHT;
use crate::scene::Scene;
use crate::{Pick, PointerButton, ViewMode};

/// Distance (CSS pixels) a press may move and still be a click.
const CLICK_SLOP: f32 = 5.0;
/// Zoom factor per wheel pixel (`exp(delta_y × WHEEL_ZOOM)`).
const WHEEL_ZOOM: f32 = 0.0015;

/// A pressed pointer.
#[derive(Clone, Copy, Debug)]
struct Pointer {
    id: i32,
    button: PointerButton,
    start: Vec2,
    last: Vec2,
}

/// The viewer without the GPU.
#[derive(Debug, Default)]
pub struct View {
    pub camera: Camera,
    pub scene: Scene,
    /// Pressed pointers, in press order.
    pointers: Vec<Pointer>,
    /// The current gesture moved beyond [`CLICK_SLOP`] or used two pointers: no click.
    dragged: bool,
    /// The user moved the camera: resizing keeps the framing instead of fitting the room.
    moved: bool,
}

impl View {
    /// Sets the canvas size in CSS pixels; frames the room until the user moves the camera.
    pub fn resize(&mut self, width: f32, height: f32) {
        self.camera.resize(width, height);
        if !self.moved {
            self.camera.fit();
        }
    }

    /// Switches the projection, keeping target and zoom.
    pub const fn set_mode(&mut self, mode: ViewMode) {
        self.camera.mode = mode;
    }

    /// A pointer was pressed at a CSS point.
    pub fn pointer_down(&mut self, id: i32, point: Vec2, button: PointerButton) {
        self.pointers.retain(|pointer| pointer.id != id);
        if self.pointers.is_empty() {
            self.dragged = false;
        } else {
            // A second pointer: pinch, never a click.
            self.dragged = true;
        }
        self.pointers.push(Pointer {
            id,
            button,
            start: point,
            last: point,
        });
    }

    /// A pointer moved; pans, orbits or pinches when pressed.
    pub fn pointer_move(&mut self, id: i32, point: Vec2) {
        let Some(index) = self.pointers.iter().position(|pointer| pointer.id == id) else {
            return;
        };
        let pointer = self.pointers[index];
        if self.pointers.len() >= 2 {
            if index < 2 {
                let other = self.pointers[1 - index].last;
                self.pinch((pointer.last, point), other);
            }
        } else {
            if !self.dragged && pointer.start.distance(point) <= CLICK_SLOP {
                return;
            }
            self.dragged = true;
            self.moved = true;
            let pans = match self.camera.mode {
                ViewMode::TopDown => true,
                ViewMode::Perspective => pointer.button != PointerButton::Primary,
            };
            if pans {
                self.camera.pan(pointer.last, point);
            } else {
                self.camera.orbit(point - pointer.last);
            }
        }
        self.pointers[index].last = point;
    }

    /// Two-pointer gesture: one pointer moved from `moved.0` to `moved.1`, the other is at
    /// `other`. Pans by the midpoint and zooms by the distance ratio around it.
    fn pinch(&mut self, moved: (Vec2, Vec2), other: Vec2) {
        let before = (moved.0 + other) / 2.0;
        let after = (moved.1 + other) / 2.0;
        let (old, new) = (moved.0.distance(other), moved.1.distance(other));
        self.moved = true;
        self.camera.pan(before, after);
        if old > 1.0 && new > 1.0 {
            self.camera.zoom(after, old / new);
        }
    }

    /// A pointer was released; a press released without dragging is a click and returns what
    /// is under it (`None` outside the room).
    pub fn pointer_up(&mut self, id: i32, point: Vec2) -> Option<Pick> {
        let index = self.pointers.iter().position(|pointer| pointer.id == id)?;
        self.pointers.remove(index);
        let click = self.pointers.is_empty() && !self.dragged;
        click.then(|| self.pick(point)).flatten()
    }

    /// Zooms around a CSS point; positive `delta_y` (scrolling down) zooms out.
    pub fn wheel(&mut self, point: Vec2, delta_y: f32) {
        self.moved = true;
        self.camera.zoom(point, (delta_y * WHEEL_ZOOM).exp());
    }

    /// The tile under a CSS point and its objects, topmost first.
    pub fn pick(&self, point: Vec2) -> Option<Pick> {
        let (origin, direction) = self.camera.ray(point);
        let (x, y) = camera::cast(origin, direction, WALL_HEIGHT, |x, y| {
            self.scene.tile_height(x, y)
        })?;
        Some(Pick {
            x,
            y,
            objects: self.scene.objects_at((x, y)),
        })
    }
}

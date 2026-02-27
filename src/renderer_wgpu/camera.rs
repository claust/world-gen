use glam::{Mat4, Vec3};
use winit::event::{DeviceEvent, ElementState, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

#[derive(Debug, Clone, Copy)]
pub enum MoveDirection {
    Forward,
    Backward,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MoveMask(u8);

impl MoveMask {
    const NONE: Self = Self(0);
    const FORWARD: Self = Self(1 << 0);
    const LEFT: Self = Self(1 << 1);
    const BACKWARD: Self = Self(1 << 2);
    const RIGHT: Self = Self(1 << 3);

    fn from_direction(direction: MoveDirection) -> Self {
        match direction {
            MoveDirection::Forward => Self::FORWARD,
            MoveDirection::Backward => Self::BACKWARD,
            MoveDirection::Left => Self::LEFT,
            MoveDirection::Right => Self::RIGHT,
        }
    }

    fn from_local_key(code: KeyCode) -> Option<Self> {
        match code {
            KeyCode::KeyW => Some(Self::FORWARD),
            KeyCode::KeyA => Some(Self::LEFT),
            KeyCode::KeyS => Some(Self::BACKWARD),
            KeyCode::KeyD => Some(Self::RIGHT),
            _ => None,
        }
    }

    fn set(&mut self, mask: Self, pressed: bool) {
        if pressed {
            self.0 |= mask.0;
        } else {
            self.0 &= !mask.0;
        }
    }

    fn contains(self, mask: Self) -> bool {
        (self.0 & mask.0) != 0
    }

    fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Debug, Clone, Copy)]
struct MovementInputs {
    local: MoveMask,
    remote: MoveMask,
}

impl MovementInputs {
    fn new() -> Self {
        Self {
            local: MoveMask::NONE,
            remote: MoveMask::NONE,
        }
    }

    fn set_local_key(&mut self, code: KeyCode, pressed: bool) -> bool {
        let Some(mask) = MoveMask::from_local_key(code) else {
            return false;
        };
        self.local.set(mask, pressed);
        true
    }

    fn set_remote_move(&mut self, direction: MoveDirection, pressed: bool) {
        self.remote
            .set(MoveMask::from_direction(direction), pressed);
    }

    fn clear_local(&mut self) {
        self.local = MoveMask::NONE;
    }

    fn effective(&self) -> MoveMask {
        self.local.union(self.remote)
    }
}

pub struct FlyCamera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
}

impl FlyCamera {
    pub fn new(position: Vec3) -> Self {
        Self {
            position,
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: -0.25,
            fov_y_radians: 65.0f32.to_radians(),
            near: 0.1,
            far: 4000.0,
        }
    }

    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        )
        .normalize()
    }

    pub fn right(&self) -> Vec3 {
        Vec3::Y.cross(self.forward()).normalize()
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.position, self.position + self.forward(), Vec3::Y);
        let projection = Mat4::perspective_rh(self.fov_y_radians, aspect, self.near, self.far);
        projection * view
    }
}

pub struct CameraController {
    movement: MovementInputs,
    local_move_up: bool,
    local_move_down: bool,
    local_run_modifier: bool,
    mouse_delta: (f64, f64),
    pub move_speed: f32,
    pub look_sensitivity: f32,
}

impl CameraController {
    pub fn new(move_speed: f32, look_sensitivity: f32) -> Self {
        Self {
            movement: MovementInputs::new(),
            local_move_up: false,
            local_move_down: false,
            local_run_modifier: false,
            mouse_delta: (0.0, 0.0),
            move_speed,
            look_sensitivity,
        }
    }

    pub fn process_window_event(&mut self, event: &WindowEvent) -> bool {
        let WindowEvent::KeyboardInput { event, .. } = event else {
            return false;
        };

        let pressed = event.state == ElementState::Pressed;
        let PhysicalKey::Code(code) = event.physical_key else {
            return false;
        };

        if self.movement.set_local_key(code, pressed) {
            return true;
        }

        match code {
            KeyCode::Space => self.local_move_up = pressed,
            KeyCode::ShiftLeft => self.local_move_down = pressed,
            KeyCode::ControlLeft => self.local_run_modifier = pressed,
            _ => return false,
        }

        true
    }

    pub fn set_remote_move(&mut self, direction: MoveDirection, pressed: bool) {
        self.movement.set_remote_move(direction, pressed);
    }

    pub fn process_device_event(&mut self, event: &DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.mouse_delta.0 += delta.0;
            self.mouse_delta.1 += delta.1;
        }
    }

    pub fn reset_inputs(&mut self) {
        self.movement.clear_local();
        self.local_move_up = false;
        self.local_move_down = false;
        self.local_run_modifier = false;
        self.mouse_delta = (0.0, 0.0);
    }

    pub fn update_camera(&mut self, dt_seconds: f32, camera: &mut FlyCamera, focused: bool) {
        if focused {
            camera.yaw -= self.mouse_delta.0 as f32 * self.look_sensitivity;
            camera.pitch -= self.mouse_delta.1 as f32 * self.look_sensitivity;
            camera.pitch = camera.pitch.clamp(-1.54, 1.54);
        }
        self.mouse_delta = (0.0, 0.0);

        let mut direction = Vec3::ZERO;
        let forward = camera.forward();
        let flat_forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let right = camera.right();
        let movement = self.movement.effective();

        if movement.contains(MoveMask::FORWARD) {
            direction += flat_forward;
        }
        if movement.contains(MoveMask::BACKWARD) {
            direction -= flat_forward;
        }
        if movement.contains(MoveMask::RIGHT) {
            direction += right;
        }
        if movement.contains(MoveMask::LEFT) {
            direction -= right;
        }
        if self.local_move_up {
            direction += Vec3::Y;
        }
        if self.local_move_down {
            direction -= Vec3::Y;
        }

        let speed = if self.local_run_modifier {
            self.move_speed * 3.0
        } else {
            self.move_speed
        };

        camera.position += direction.normalize_or_zero() * speed * dt_seconds;
    }
}

#[cfg(test)]
mod tests {
    use super::{CameraController, FlyCamera, MoveDirection};
    use glam::Vec3;

    #[test]
    fn remote_wasd_moves_camera_and_reset_inputs_does_not_clear_remote_state() {
        let mut controller = CameraController::new(10.0, 0.0);
        let mut camera = FlyCamera::new(Vec3::ZERO);

        controller.set_remote_move(MoveDirection::Forward, true);
        controller.update_camera(1.0, &mut camera, false);
        let first_distance = camera.position.length();
        assert!(first_distance > 0.0);

        controller.reset_inputs();
        controller.update_camera(1.0, &mut camera, false);
        let second_distance = camera.position.length();
        assert!(second_distance > first_distance);

        controller.set_remote_move(MoveDirection::Forward, false);
        let before = camera.position;
        controller.update_camera(1.0, &mut camera, false);
        assert!((camera.position - before).length() < 1e-6);
    }
}

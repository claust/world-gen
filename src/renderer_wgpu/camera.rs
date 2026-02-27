use glam::{Mat4, Vec3};
use winit::event::{DeviceEvent, ElementState, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

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
    move_forward: bool,
    move_back: bool,
    move_left: bool,
    move_right: bool,
    move_up: bool,
    move_down: bool,
    run_modifier: bool,
    mouse_delta: (f64, f64),
    pub move_speed: f32,
    pub look_sensitivity: f32,
}

impl CameraController {
    pub fn new(move_speed: f32, look_sensitivity: f32) -> Self {
        Self {
            move_forward: false,
            move_back: false,
            move_left: false,
            move_right: false,
            move_up: false,
            move_down: false,
            run_modifier: false,
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

        match code {
            KeyCode::KeyW => self.move_forward = pressed,
            KeyCode::KeyS => self.move_back = pressed,
            KeyCode::KeyA => self.move_left = pressed,
            KeyCode::KeyD => self.move_right = pressed,
            KeyCode::Space => self.move_up = pressed,
            KeyCode::ShiftLeft => self.move_down = pressed,
            KeyCode::ControlLeft => self.run_modifier = pressed,
            _ => return false,
        }

        true
    }

    pub fn process_device_event(&mut self, event: &DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.mouse_delta.0 += delta.0;
            self.mouse_delta.1 += delta.1;
        }
    }

    pub fn reset_inputs(&mut self) {
        self.move_forward = false;
        self.move_back = false;
        self.move_left = false;
        self.move_right = false;
        self.move_up = false;
        self.move_down = false;
        self.run_modifier = false;
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

        if self.move_forward {
            direction += flat_forward;
        }
        if self.move_back {
            direction -= flat_forward;
        }
        if self.move_right {
            direction += right;
        }
        if self.move_left {
            direction -= right;
        }
        if self.move_up {
            direction += Vec3::Y;
        }
        if self.move_down {
            direction -= Vec3::Y;
        }

        let speed = if self.run_modifier {
            self.move_speed * 3.0
        } else {
            self.move_speed
        };

        camera.position += direction.normalize_or_zero() * speed * dt_seconds;
    }
}

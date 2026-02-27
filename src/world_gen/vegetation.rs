#![allow(dead_code)]

use glam::Vec3;

#[derive(Debug, Clone, Copy)]
pub enum Species {
    Pine,
    Broadleaf,
}

#[derive(Debug, Clone, Copy)]
pub struct VegetationInstance {
    pub position: Vec3,
    pub rotation_y: f32,
    pub scale: f32,
    pub species: Species,
}

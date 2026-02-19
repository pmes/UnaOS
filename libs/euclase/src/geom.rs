use crate::vec::Vec3;
use crate::ray::Ray;
use bytemuck::{Pod, Zeroable};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct AABB {
    pub min: Vec3,
    pub max: Vec3,
}

impl Sphere {
    #[inline]
    pub const fn new(center: Vec3, radius: f32) -> Self {
        Self { center, radius }
    }

    #[inline]
    pub fn intersect(&self, ray: &Ray) -> Option<f32> {
        let oc = ray.origin - self.center;
        let a = ray.dir.length_squared();
        let half_b = oc.dot(ray.dir);
        let c = oc.length_squared() - self.radius * self.radius;
        let discriminant = half_b * half_b - a * c;

        if discriminant < 0.0 {
            return None;
        }

        let sqrtd = libm::sqrtf(discriminant);
        let root = (-half_b - sqrtd) / a;

        if root > 0.001 { // Check t_min
            return Some(root);
        }

        let root = (-half_b + sqrtd) / a;
         if root > 0.001 {
            return Some(root);
        }

        None
    }
}

impl AABB {
    #[inline]
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    #[inline]
    pub fn intersect(&self, ray: &Ray) -> Option<f32> {
        let mut t_min = -core::f32::INFINITY;
        let mut t_max = core::f32::INFINITY;

        // X
        if ray.dir.x != 0.0 {
            let inv_d = 1.0 / ray.dir.x;
            let mut t0 = (self.min.x - ray.origin.x) * inv_d;
            let mut t1 = (self.max.x - ray.origin.x) * inv_d;
            if inv_d < 0.0 {
                 let temp = t0; t0 = t1; t1 = temp;
            }
            if t0 > t_min { t_min = t0; }
            if t1 < t_max { t_max = t1; }
            if t_max < t_min { return None; }
        } else {
            if ray.origin.x < self.min.x || ray.origin.x > self.max.x {
                return None;
            }
        }

        // Y
        if ray.dir.y != 0.0 {
            let inv_d = 1.0 / ray.dir.y;
            let mut t0 = (self.min.y - ray.origin.y) * inv_d;
            let mut t1 = (self.max.y - ray.origin.y) * inv_d;
            if inv_d < 0.0 {
                 let temp = t0; t0 = t1; t1 = temp;
            }
            if t0 > t_min { t_min = t0; }
            if t1 < t_max { t_max = t1; }
            if t_max < t_min { return None; }
        } else {
             if ray.origin.y < self.min.y || ray.origin.y > self.max.y {
                return None;
            }
        }

        // Z
        if ray.dir.z != 0.0 {
            let inv_d = 1.0 / ray.dir.z;
            let mut t0 = (self.min.z - ray.origin.z) * inv_d;
            let mut t1 = (self.max.z - ray.origin.z) * inv_d;
            if inv_d < 0.0 {
                 let temp = t0; t0 = t1; t1 = temp;
            }
            if t0 > t_min { t_min = t0; }
            if t1 < t_max { t_max = t1; }
            if t_max < t_min { return None; }
        } else {
             if ray.origin.z < self.min.z || ray.origin.z > self.max.z {
                return None;
            }
        }

        if t_min > 0.001 {
            Some(t_min)
        } else if t_max > 0.001 {
             Some(t_max) // Inside the AABB
        } else {
            None
        }
    }
}

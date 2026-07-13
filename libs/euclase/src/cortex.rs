// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::sync::Arc;
use wgpu::{
    Backends, Device, DeviceDescriptor, Features, Instance, InstanceDescriptor, Limits,
    PowerPreference, Queue, RequestAdapterOptions, Surface,
};

/// What a consumer needs from the GPU: which device features to demand and how
/// to pace presentation. Different consumers want opposite things — vug's CAD
/// wireframes require `POLYGON_MODE_LINE` and deliberately tear
/// (`AutoNoVsync`, its go-fast choice), while a viewer like `facet` needs no
/// extra features and wants vsync. Parameterizing here keeps one `Cortex` for
/// both instead of hard-coding either taste.
#[derive(Clone, Copy, Debug)]
pub struct CortexSpec {
    /// Device features to hard-require at `request_device` time.
    pub required_features: Features,
    /// The surface present mode configured for the swapchain.
    pub present_mode: wgpu::PresentMode,
}

impl CortexSpec {
    /// vug's wireframe spec: line polygon mode, tear-if-you-must no-vsync.
    /// This is the historical `Cortex::ignite` behavior, preserved verbatim.
    pub fn wireframe() -> Self {
        Self {
            required_features: Features::POLYGON_MODE_LINE,
            present_mode: wgpu::PresentMode::AutoNoVsync,
        }
    }

    /// A presentation/viewer spec: no extra device features, vsync pacing.
    /// The right default for on-demand redraw surfaces (e.g. `facet`).
    pub fn presentation() -> Self {
        Self {
            required_features: Features::empty(),
            present_mode: wgpu::PresentMode::AutoVsync,
        }
    }
}

/// Why the cortex failed to ignite. Consumers with a fallback path (facet's
/// CPU blit) match on this instead of dying; `ignite` keeps its historical
/// panic behavior on top of these.
#[derive(Debug)]
pub enum CortexError {
    /// The instance refused to create a surface for the given target.
    Surface(wgpu::CreateSurfaceError),
    /// No adapter satisfied the request (headless CI, exotic GPUs).
    NoAdapter(wgpu::RequestAdapterError),
    /// The adapter refused the device request (missing features/limits).
    Device(wgpu::RequestDeviceError),
}

impl core::fmt::Display for CortexError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Surface(e) => write!(f, "surface creation failed: {e}"),
            Self::NoAdapter(e) => write!(f, "no suitable GPU adapter: {e}"),
            Self::Device(e) => write!(f, "device request failed: {e}"),
        }
    }
}

impl std::error::Error for CortexError {}

pub struct Cortex<'a> {
    pub instance: Instance,
    pub surface: Surface<'a>,
    pub device: Arc<Device>,
    pub queue: Arc<Queue>,
    pub config: wgpu::SurfaceConfiguration,
}

impl<'a> Cortex<'a> {
    /// Ignites the visual cortex. Binds to the Quartzite-provided window.
    ///
    /// Historical entry point (vug's): wireframe spec, panics on failure.
    pub async fn ignite(
        window: impl Into<wgpu::SurfaceTarget<'a>>,
        width: u32,
        height: u32,
    ) -> Self {
        let instance = new_instance();

        let surface = instance
            .create_surface(window)
            .expect("Surface creation failed. Quartzite betrayed us.");

        match Self::bind(instance, surface, width, height, CortexSpec::wireframe()).await {
            Ok(cortex) => cortex,
            Err(CortexError::NoAdapter(_)) => {
                panic!("Silicon rejected the adapter request. Engine stalled.")
            }
            Err(CortexError::Device(_)) => {
                panic!("Device request failed. Insufficient GPU authority.")
            }
            Err(CortexError::Surface(_)) => {
                unreachable!("surface already created above")
            }
        }
    }

    /// Ignite onto an existing `CAMetalLayer`, blocking until the GPU answers.
    ///
    /// This is the vessel-bootstrap entry point: AppKit hands quartzite a
    /// layer-hosting view on the main thread, synchronously, with no
    /// `raw-window-handle` implementor in sight — so we take the layer pointer
    /// straight through wgpu's unsafe Metal target and block on the async
    /// adapter/device handshake with `pollster` (main-thread-safe; there is no
    /// executor underneath a vessel's AppKit bootstrap).
    ///
    /// Returns `Err` instead of panicking so callers can fall back (facet
    /// falls back to its CPU blit path).
    ///
    /// # Safety
    ///
    /// - `layer` must be a valid, retained `CAMetalLayer` pointer.
    /// - The layer must outlive the returned `Cortex<'static>`. The intended
    ///   ownership shape (quartzite's `FacetGpuView`) guarantees this: the
    ///   view owns the layer (as its backing layer) *and* owns the `Cortex`
    ///   (boxed inside its ivars); `define_class!` drops the Rust ivars —
    ///   and with them the `Cortex` and its `Surface` — before the NSView
    ///   superclass `dealloc` releases the layer.
    pub unsafe fn ignite_layer_blocking(
        layer: *mut core::ffi::c_void,
        width: u32,
        height: u32,
        spec: CortexSpec,
    ) -> Result<Cortex<'static>, CortexError> {
        let instance = new_instance();

        let surface = unsafe {
            instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }
        .map_err(CortexError::Surface)?;

        pollster::block_on(Cortex::bind(instance, surface, width, height, spec))
    }

    /// The shared fallible core: adapter, device, and swapchain configuration
    /// for an already-created surface, per `spec`.
    async fn bind(
        instance: Instance,
        surface: Surface<'a>,
        width: u32,
        height: u32,
        spec: CortexSpec,
    ) -> Result<Self, CortexError> {
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .map_err(CortexError::NoAdapter)?;

        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("Euclase_Primary_Cortex"),
                required_features: spec.required_features,
                required_limits: Limits::default(),
                ..Default::default()
            })
            .await
            .map_err(CortexError::Device)?;

        // Prefer an sRGB swapchain format: shader outputs are linear and the
        // hardware encodes on store, which is the color-managed path.
        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: spec.present_mode,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&device, &config);

        Ok(Self {
            instance,
            surface,
            device: Arc::new(device),
            queue: Arc::new(queue),
            config,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }
}

fn new_instance() -> Instance {
    Instance::new(&InstanceDescriptor {
        backends: Backends::VULKAN | Backends::METAL, // Legacy is dead to us.
        ..Default::default()
    })
}

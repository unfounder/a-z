// Vulturine
// Multi-rendering coordinator for Software, Servo bridge, and Ladybird bridge.

mod external;
pub mod layout;
pub mod renderer;

use crate::dom::document::Document;
use std::sync::{Arc, Mutex};

/// Available rendering backends.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderBackend {
    /// Pure-Rust parallel engine target.
    Servo,
    /// LibWeb/Ladybird target.
    Ladybird,
    /// In-process fallback renderer.
    Software,
}

impl Default for RenderBackend {
    fn default() -> Self {
        Self::Software
    }
}

/// Plain RGBA pixel buffer (no egui dependency).
#[derive(Clone, Default)]
pub struct RgbaImage {
    pub size:   [usize; 2],
    pub pixels: Vec<[u8; 4]>,
}

/// Result of a render call: RGBA image + dimensions + link hitboxes.
#[derive(Clone)]
pub struct RenderOutput {
    pub image:    RgbaImage,
    pub width:    u32,
    pub height:   u32,
    pub hitboxes: Vec<renderer::HitBox>,
}

/// Vulturine runtime engine coordinator.
pub struct VulturineEngine {
    pub active_backend: Arc<Mutex<RenderBackend>>,
}

impl VulturineEngine {
    pub fn new(default_backend: RenderBackend) -> Self {
        log::info!("Vulturine: initialising with {:?} backend", default_backend);
        Self {
            active_backend: Arc::new(Mutex::new(default_backend)),
        }
    }

    /// Hot-swap the rendering backend at runtime.
    pub fn switch_engine(&self, new_backend: RenderBackend) {
        log::info!("Vulturine: switching to {:?}", new_backend);
        *self.active_backend.lock().unwrap() = new_backend;
    }

    pub fn current_backend(&self) -> RenderBackend {
        self.active_backend.lock().unwrap().clone()
    }

    pub fn is_backend_configured(&self, backend: RenderBackend) -> bool {
        external::is_configured(backend)
    }

    /// Render a document into a pixel buffer.
    /// `width` / `height` are the target canvas card dimensions in physical pixels.
    pub fn render_document(
        &self,
        document: &Document,
        url: &str,
        width: u32,
        height: u32,
    ) -> RenderOutput {
        let backend = self.current_backend();
        match backend {
            RenderBackend::Servo => {
                if let Some(render) =
                    external::try_render(external::ExternalKind::Servo, url, width, height)
                {
                    return render;
                }
                if let Some(env_name) = external::config_hint(RenderBackend::Servo) {
                    log::debug!(
                        "Vulturine/Servo: external bridge unavailable (set {}), using Software fallback",
                        env_name
                    );
                }
                renderer::software_render(document, url, width, height)
            }
            RenderBackend::Ladybird => {
                if let Some(render) =
                    external::try_render(external::ExternalKind::Ladybird, url, width, height)
                {
                    return render;
                }
                if let Some(env_name) = external::config_hint(RenderBackend::Ladybird) {
                    log::debug!(
                        "Vulturine/Ladybird: external bridge unavailable (set {}), using Software fallback",
                        env_name
                    );
                }
                renderer::software_render(document, url, width, height)
            }
            RenderBackend::Software => renderer::software_render(document, url, width, height),
        }
    }

    /// Decode and route a video stream frame.
    pub fn route_video_frame(&self, video_id: &str, raw_bytes: &[u8]) {
        log::debug!(
            "Vulturine: video frame {} bytes (id={})",
            raw_bytes.len(),
            video_id
        );
        // TODO: hardware decode -> WGPU texture upload
    }
}

//! Rust-owned wgpu viewport window.
//!
//! Tauri's webview can't host a `wgpu::Surface` directly, so we open a
//! second native window (via `winit`) on a dedicated thread and paint it
//! with wgpu.
//!
//! Talks to the rest of the app through one `winit` user event,
//! [`UserEvent::LoadScene`], sent via [`EventLoopProxy::send_event`].
//! The proxy is stashed in [`VIEWPORT_PROXY`] when the viewport thread
//! starts, so Tauri command handlers (on a different thread) can push
//! scenes in.
//!
//! H2a: replaced the hardcoded rectangle with polygons loaded from a
//! `.gds` file. Polygons are ear-clipped into triangles and coloured
//! by layer; the ortho projection fits the loaded bbox.

use std::sync::{Arc, Mutex, OnceLock};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

use crate::gds::{self, Polygon};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    color: [f32; 3],
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x3];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Uniforms {
    proj: [[f32; 4]; 4],
}

fn ortho(left: f32, right: f32, bottom: f32, top: f32) -> [[f32; 4]; 4] {
    let rl = right - left;
    let tb = top - bottom;
    [
        [ 2.0 / rl,           0.0,                  0.0, 0.0],
        [ 0.0,                2.0 / tb,             0.0, 0.0],
        [ 0.0,                0.0,                  1.0, 0.0],
        [-(right + left)/rl, -(top + bottom)/tb,    0.0, 1.0],
    ]
}

/// Deterministic colour per GDS layer. H2c will replace this with a
/// proper technology-file map.
fn layer_color(layer: i16) -> [f32; 3] {
    // Small palette for the typical 0..7 process layers, then a
    // hash-based fallback so unknown layers still render distinctly.
    const PALETTE: [[f32; 3]; 8] = [
        [0.55, 0.55, 0.55], // 0  background-ish
        [0.45, 0.85, 0.40], // 1  poly (green)
        [0.30, 0.55, 0.95], // 2  active/diff (blue)
        [0.95, 0.85, 0.30], // 3  contact (yellow)
        [0.90, 0.40, 0.40], // 4  metal1 (red)
        [0.40, 0.85, 0.85], // 5  metal2 (cyan)
        [0.85, 0.50, 0.85], // 6  metal3 (magenta)
        [0.95, 0.65, 0.30], // 7  via (orange)
    ];
    if (0..PALETTE.len() as i16).contains(&layer) {
        return PALETTE[layer as usize];
    }
    // FNV-ish hash to a pastel.
    let h = (layer as i32 as u32).wrapping_mul(2654435761);
    let r = ((h >> 16) & 0xff) as f32 / 255.0;
    let g = ((h >> 8) & 0xff) as f32 / 255.0;
    let b = (h & 0xff) as f32 / 255.0;
    [0.4 + 0.5 * r, 0.4 + 0.5 * g, 0.4 + 0.5 * b]
}

/// Triangulate one polygon (no holes for now) into vertex+index buffers.
fn tessellate(poly: &Polygon, verts: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    if poly.points.len() < 3 {
        return;
    }
    // Flatten coords for earcutr.
    let mut flat: Vec<f64> = Vec::with_capacity(poly.points.len() * 2);
    for p in &poly.points {
        flat.push(p[0]);
        flat.push(p[1]);
    }
    let Ok(tris) = earcutr::earcut(&flat, &[], 2) else {
        log::warn!("earcut failed on polygon with {} pts", poly.points.len());
        return;
    };
    let base = verts.len() as u32;
    let color = layer_color(poly.layer);
    for p in &poly.points {
        verts.push(Vertex { pos: [p[0] as f32, p[1] as f32], color });
    }
    for i in tris {
        indices.push(base + i as u32);
    }
}

struct Scene {
    verts: Vec<Vertex>,
    indices: Vec<u32>,
    bbox: gds::Bbox,
}

impl Scene {
    fn from_polygons(polys: &[Polygon]) -> Self {
        let bbox = gds::bbox(polys);
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        for p in polys {
            tessellate(p, &mut verts, &mut indices);
        }
        Self { verts, indices, bbox }
    }
}

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    /// Latest uploaded scene. None until first LoadScene.
    scene_buffers: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    /// World-space view: half-height around `view_center`; half-width
    /// is derived from surface aspect.
    view_center: [f32; 2],
    view_half_h: f32,
    /// Cached bbox of the loaded scene (used by fit).
    scene_bbox: gds::Bbox,
    /// Cursor position in physical pixels (top-left origin).
    cursor_px: [f32; 2],
    /// Middle-button drag state.
    panning: bool,
    pan_last_px: [f32; 2],
    window: Arc<Window>,
}

impl GpuState {
    async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).expect("create surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no compatible GPU adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("gdssim-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gdssim-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
        });

        // Default world view: 400×300 centred on origin until a scene loads.
        let view_center = [0.0_f32, 0.0_f32];
        let view_half_h = 150.0_f32;
        let uniforms = Uniforms {
            proj: ortho_for_view(view_center, view_half_h, size.width as f32, size.height as f32),
        };
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gdssim-uniform-buf"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gdssim-uniform-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gdssim-uniform-bg"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gdssim-pipeline-layout"),
            bind_group_layouts: &[&uniform_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gdssim-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Vertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            uniform_buf,
            uniform_bg,
            scene_buffers: None,
            view_center,
            view_half_h,
            scene_bbox: gds::Bbox { min: [0.0; 2], max: [0.0; 2] },
            cursor_px: [0.0, 0.0],
            panning: false,
            pan_last_px: [0.0, 0.0],
            window,
        }
    }

    fn upload_scene(&mut self, scene: Scene) {
        if scene.indices.is_empty() {
            self.scene_buffers = None;
            return;
        }
        let vbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gdssim-scene-vbuf"),
            contents: bytemuck::cast_slice(&scene.verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gdssim-scene-ibuf"),
            contents: bytemuck::cast_slice(&scene.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let n = scene.indices.len() as u32;
        self.scene_buffers = Some((vbuf, ibuf, n));
        self.scene_bbox = scene.bbox;
        self.fit_view();
    }

    /// Centre + zoom the view to fit the loaded scene bbox with 10% pad.
    fn fit_view(&mut self) {
        let b = self.scene_bbox;
        if b.min[0] >= b.max[0] || b.min[1] >= b.max[1] {
            return;
        }
        let cx = ((b.min[0] + b.max[0]) * 0.5) as f32;
        let cy = ((b.min[1] + b.max[1]) * 0.5) as f32;
        let w = (b.max[0] - b.min[0]) as f32;
        let h = (b.max[1] - b.min[1]) as f32;
        let aspect = self.aspect().max(1e-6);
        let half_h = (h * 0.5).max((w / aspect) * 0.5) * 1.1; // 10% pad
        self.view_center = [cx, cy];
        self.view_half_h = half_h.max(1.0);
        self.write_proj();
    }

    fn aspect(&self) -> f32 {
        if self.config.height > 0 {
            self.config.width as f32 / self.config.height as f32
        } else {
            1.0
        }
    }

    fn view_half_w(&self) -> f32 {
        self.view_half_h * self.aspect()
    }

    /// Convert a physical-pixel position to world coords under the
    /// current view.
    fn pixel_to_world(&self, px: [f32; 2]) -> [f32; 2] {
        let w = self.config.width.max(1) as f32;
        let h = self.config.height.max(1) as f32;
        let nx = (px[0] / w) * 2.0 - 1.0;
        let ny = 1.0 - (px[1] / h) * 2.0;
        [
            self.view_center[0] + nx * self.view_half_w(),
            self.view_center[1] + ny * self.view_half_h,
        ]
    }

    /// Zoom by `factor` (>1 zooms out, <1 zooms in) anchored on the
    /// world point under `px`.
    fn zoom_at(&mut self, px: [f32; 2], factor: f32) {
        let before = self.pixel_to_world(px);
        let new_half = (self.view_half_h * factor).clamp(1e-3, 1e12);
        self.view_half_h = new_half;
        let after = self.pixel_to_world(px);
        self.view_center[0] += before[0] - after[0];
        self.view_center[1] += before[1] - after[1];
        self.write_proj();
    }

    /// Pan by a pixel delta (cursor moved by `(dx, dy)` while panning);
    /// world moves opposite so content tracks the cursor.
    fn pan_pixels(&mut self, dx: f32, dy: f32) {
        let w = self.config.width.max(1) as f32;
        let h = self.config.height.max(1) as f32;
        let dwx = (dx / w) * 2.0 * self.view_half_w();
        let dwy = (dy / h) * 2.0 * self.view_half_h;
        self.view_center[0] -= dwx;
        self.view_center[1] += dwy; // pixel y down ↔ world y up
        self.write_proj();
    }

    fn write_proj(&self) {
        let uniforms = Uniforms {
            proj: ortho_for_view(
                self.view_center,
                self.view_half_h,
                self.config.width as f32,
                self.config.height as f32,
            ),
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[uniforms]));
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.write_proj();
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gdssim-encoder"),
            });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gdssim-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.06,
                            g: 0.06,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some((vbuf, ibuf, n)) = &self.scene_buffers {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.uniform_bg, &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*n, 0, 0..1);
            }
        }
        self.queue.submit(std::iter::once(enc.finish()));
        frame.present();
        Ok(())
    }
}

/// Build an ortho matrix centred on `center` with the given world
/// half-height, aspect-corrected for the current surface.
fn ortho_for_view(center: [f32; 2], half_h: f32, w: f32, h: f32) -> [[f32; 4]; 4] {
    let aspect = if h > 0.0 { w / h } else { 1.0 };
    let hh = half_h.max(1e-3);
    let hw = hh * aspect;
    ortho(center[0] - hw, center[0] + hw, center[1] - hh, center[1] + hh)
}

#[derive(Debug)]
pub enum UserEvent {
    LoadScene(Vec<Polygon>),
}

struct App {
    state: Option<GpuState>,
    pending_scene: Option<Vec<Polygon>>,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("GDSSIM — GPU Viewport")
            .with_inner_size(winit::dpi::LogicalSize::new(800.0, 600.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("create viewport window"),
        );
        let mut state = pollster::block_on(GpuState::new(window));
        if let Some(polys) = self.pending_scene.take() {
            state.upload_scene(Scene::from_polygons(&polys));
        }
        self.state = Some(state);
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::LoadScene(polys) => match self.state.as_mut() {
                Some(state) => {
                    state.upload_scene(Scene::from_polygons(&polys));
                    state.window.request_redraw();
                }
                None => {
                    self.pending_scene = Some(polys);
                }
            },
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::CursorMoved { position, .. } => {
                let p = [position.x as f32, position.y as f32];
                if state.panning {
                    let dx = p[0] - state.pan_last_px[0];
                    let dy = p[1] - state.pan_last_px[1];
                    state.pan_pixels(dx, dy);
                }
                state.cursor_px = p;
                state.pan_last_px = p;
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                if button == MouseButton::Middle {
                    state.panning = btn_state == ElementState::Pressed;
                    state.pan_last_px = state.cursor_px;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 60.0,
                };
                // Wheel up (positive) zooms in → factor < 1.
                let factor = (-lines * 0.15).exp();
                state.zoom_at(state.cursor_px, factor);
            }
            WindowEvent::KeyboardInput { event: key_event, .. } => {
                if key_event.state == ElementState::Pressed {
                    if let PhysicalKey::Code(code) = key_event.physical_key {
                        match code {
                            KeyCode::KeyF => state.fit_view(),
                            KeyCode::Equal | KeyCode::NumpadAdd => {
                                let c = [state.config.width as f32 * 0.5,
                                         state.config.height as f32 * 0.5];
                                state.zoom_at(c, 0.8);
                            }
                            KeyCode::Minus | KeyCode::NumpadSubtract => {
                                let c = [state.config.width as f32 * 0.5,
                                         state.config.height as f32 * 0.5];
                                state.zoom_at(c, 1.25);
                            }
                            _ => {}
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                match state.render() {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        let s = state.window.inner_size();
                        state.resize(s.width, s.height);
                    }
                    Err(e) => log::error!("render error: {e:?}"),
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

/// Set once when the viewport thread starts; used by Tauri command
/// handlers (on a separate thread) to push scenes into the event loop.
pub static VIEWPORT_PROXY: OnceLock<Mutex<Option<EventLoopProxy<UserEvent>>>> = OnceLock::new();

/// Open the viewport window on a dedicated OS thread.
/// Returns immediately; the window keeps running until the user closes it.
pub fn spawn() -> Result<(), String> {
    let slot = VIEWPORT_PROXY.get_or_init(|| Mutex::new(None));
    {
        let guard = slot.lock().unwrap();
        if guard.is_some() {
            return Err("viewport already open".into());
        }
    }
    std::thread::Builder::new()
        .name("gdssim-viewport".into())
        .spawn(|| {
            let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
                Ok(el) => el,
                Err(e) => {
                    log::error!("event loop init failed: {e:?}");
                    return;
                }
            };
            let proxy = event_loop.create_proxy();
            if let Some(slot) = VIEWPORT_PROXY.get() {
                *slot.lock().unwrap() = Some(proxy);
            }
            let mut app = App { state: None, pending_scene: None };
            if let Err(e) = event_loop.run_app(&mut app) {
                log::error!("event loop exited with error: {e:?}");
            }
            // Window closed; clear the proxy so the user can reopen.
            if let Some(slot) = VIEWPORT_PROXY.get() {
                *slot.lock().unwrap() = None;
            }
        })
        .map_err(|e| format!("spawn viewport thread: {e}"))?;
    Ok(())
}

/// Push a parsed polygon set to the running viewport.
pub fn send_scene(polys: Vec<Polygon>) -> Result<(), String> {
    let slot = VIEWPORT_PROXY.get().ok_or("viewport not started")?;
    let guard = slot.lock().unwrap();
    let proxy = guard.as_ref().ok_or("viewport not started")?;
    proxy
        .send_event(UserEvent::LoadScene(polys))
        .map_err(|_| "viewport event loop closed".to_string())
}

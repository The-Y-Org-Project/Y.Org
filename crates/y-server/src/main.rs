mod wayland;
mod window;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle};
use smithay::backend::input::{ButtonState, KeyState};
use smithay::reexports::wayland_server::backend::ObjectId;
use std::time::{Duration, Instant};
use wayland::WaylandServer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::window::{Window, WindowId};
use window::WindowStack;
use y_gpu::{GpuContext, Renderer, Surface};

const BACKGROUND: [f32; 4] = [0.08, 0.05, 0.18, 1.0];
const SOCKET_NAME: &str = "wayland-y";

fn build_stack() -> WindowStack {
    let mut stack = WindowStack::default();
    let blue = stack.add(60.0, 60.0, 360.0, 260.0, [0.2, 0.4, 0.9]);
    stack.add(260.0, 160.0, 320.0, 240.0, [0.2, 0.75, 0.4]);
    stack.add(440.0, 80.0, 280.0, 220.0, [0.95, 0.6, 0.15]);
    stack.raise(blue);
    stack
}

fn demo_content(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let fx = x as f32 / width as f32;
            let fy = y as f32 / height as f32;
            let (r, g, b) = match seed % 3 {
                0 => (fx, fy, 1.0 - fx),
                1 => (1.0 - fy, fx, fy),
                _ => (fx, 1.0 - fx, fy),
            };
            let shade = if x % 40 == 0 || y % 40 == 0 {
                0.35
            } else {
                1.0
            };
            pixels.extend_from_slice(&[
                (r * shade * 255.0) as u8,
                (g * shade * 255.0) as u8,
                (b * shade * 255.0) as u8,
                255,
            ]);
        }
    }
    pixels
}

struct App {
    renderer: Option<Renderer>,
    surface: Option<Surface>,
    window: Option<Window>,
    gpu: GpuContext,
    display: RawDisplayHandle,
    stack: WindowStack,
    cursor: (f32, f32),
    drag: Option<(u32, f32, f32)>,
    wayland: WaylandServer,
    mapped: Vec<(ObjectId, u32)>,
    started: Instant,
}

impl App {
    fn resize_to_window(&mut self) {
        if let (Some(renderer), Some(surface), Some(window)) =
            (&mut self.renderer, &self.surface, &self.window)
        {
            let size = window.inner_size();
            renderer
                .resize(surface, (size.width, size.height))
                .expect("failed to resize swapchain");
        }
    }

    fn upload_scene(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            let (vertices, draws) = self.stack.scene();
            renderer
                .set_scene(&vertices, &draws)
                .expect("failed to upload scene");
        }
    }

    fn pump_wayland(&mut self) {
        self.wayland
            .dispatch()
            .expect("failed to dispatch Wayland clients");

        let alive = self.wayland.toplevel_ids();
        let mut changed = false;

        let mut gone = Vec::new();
        self.mapped.retain(|(surface_id, window_id)| {
            let keep = alive.contains(surface_id);
            if !keep {
                gone.push(*window_id);
            }
            keep
        });
        for window_id in gone {
            self.stack.remove(window_id);
            if matches!(self.drag, Some((id, _, _)) if id == window_id) {
                self.drag = None;
            }
            println!("Y: Wayland toplevel closed, window {window_id} removed");
            changed = true;
        }

        for surface_id in alive {
            if self.mapped.iter().any(|(known, _)| *known == surface_id) {
                continue;
            }
            let n = self.mapped.len() as f32;
            let id = self.stack.add(
                80.0 + 40.0 * n,
                80.0 + 30.0 * n,
                400.0,
                300.0,
                [0.45, 0.45, 0.5],
            );
            self.stack.raise(id);
            self.mapped.push((surface_id, id));
            println!("Y: new Wayland toplevel mapped as window {id}");
            changed = true;
        }

        for (surface_id, pixels) in self.wayland.take_updates() {
            let Some(window_id) = self
                .mapped
                .iter()
                .find(|(known, _)| *known == surface_id)
                .map(|(_, id)| *id)
            else {
                continue;
            };
            let Some(renderer) = self.renderer.as_mut() else {
                continue;
            };

            match self.stack.texture_of(window_id) {
                Some(texture) => renderer
                    .update_texture(texture, pixels.width, pixels.height, &pixels.rgba)
                    .expect("failed to update window texture"),
                None => {
                    let texture = renderer
                        .create_texture(pixels.width, pixels.height, &pixels.rgba)
                        .expect("failed to create window texture");
                    self.stack.set_texture(window_id, texture);
                    changed = true;
                }
            }

            if self
                .stack
                .set_body_size(window_id, pixels.width, pixels.height)
            {
                changed = true;
            }
        }

        if changed {
            self.upload_scene();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window = event_loop
            .create_window(Window::default_attributes().with_title("Y.Org - test"))
            .expect("failed to create window");

        let handle = window
            .window_handle()
            .expect("failed to get window handle")
            .as_raw();
        let surface = self
            .gpu
            .create_surface(self.display, handle)
            .expect("failed to create Vulkan surface");

        let gpu_index: usize = std::env::var("Y_GPU")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let size = window.inner_size();
        let mut renderer = self
            .gpu
            .create_renderer(&surface, (size.width, size.height), gpu_index)
            .expect("failed to create renderer");

        for (id, width, height) in self.stack.pending_textures() {
            let pixels = demo_content(width, height, id);
            let texture = renderer
                .create_texture(width, height, &pixels)
                .expect("failed to create window texture");
            self.stack.set_texture(id, texture);
        }

        println!(
            "Y: renderer ready | GPU: {} | format {:?} | {}x{} | {} swapchain images",
            renderer.gpu_name,
            renderer.format,
            renderer.extent.width,
            renderer.extent.height,
            renderer.images.len(),
        );

        window.request_redraw();

        self.renderer = Some(renderer);
        self.surface = Some(surface);
        self.window = Some(window);
        self.upload_scene();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.pump_wayland();
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(8),
        ));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("Y: shutting down...");
                event_loop.exit();
            }
            WindowEvent::Resized(_) => {
                self.resize_to_window();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                let time = self.started.elapsed().as_millis() as u32;
                self.wayland.pointer_motion(self.cursor.0, self.cursor.1, time);

                if let Some((id, dx, dy)) = self.drag {
                    self.stack.move_to(id, self.cursor.0 - dx, self.cursor.1 - dy);
                    self.upload_scene();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                let time = self.started.elapsed().as_millis() as u32;
                let btn_state = if state == ElementState::Pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };

                match state {
                    ElementState::Pressed => {
                        if let Some((id, on_title)) =
                            self.stack.hit_test(self.cursor.0, self.cursor.1)
                        {
                            self.stack.raise(id);

                            if let Some((surface_id, _)) =
                                self.mapped.iter().find(|(_, wid)| *wid == id)
                            {
                                self.wayland.set_focus(surface_id.clone(), time);
                            }

                            if on_title {
                                if let Some((wx, wy)) = self.stack.position(id) {
                                    self.drag = Some((id, self.cursor.0 - wx, self.cursor.1 - wy));
                                }
                            } else {
                                self.wayland.pointer_button(0x110, btn_state, time);
                            }
                            self.upload_scene();
                        }
                    }
                    ElementState::Released => {
                        if self.drag.is_some() {
                            self.drag = None;
                        } else {
                            self.wayland.pointer_button(0x110, btn_state, time);
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(scancode) = code.to_scancode() {
                        let time = self.started.elapsed().as_millis() as u32;
                        let key_state = if event.state == ElementState::Pressed {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        };
                        self.wayland.key_input(scancode, key_state, time);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.wayland
                    .send_frames(self.started.elapsed().as_millis() as u32);
                let stale = match &self.renderer {
                    Some(renderer) => renderer
                        .draw_frame(BACKGROUND)
                        .expect("failed to draw frame"),
                    None => false,
                };
                if stale {
                    self.resize_to_window();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() -> y_gpu::Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let display = event_loop.display_handle()?.as_raw();
    let gpu = GpuContext::new(display)?;

    let gpus = gpu.list_gpus()?;
    println!("Y: {} GPU(s) found", gpus.len());
    for (i, g) in gpus.iter().enumerate() {
        let (a, b, c) = g.vulkan_version;
        println!(
            "  [{}] {} ({:?}) - Vulkan {}.{}.{}",
            i, g.name, g.device_type, a, b, c
        );
    }

    let wayland = WaylandServer::new(SOCKET_NAME)?;
    println!("Y: Wayland socket ready, use WAYLAND_DISPLAY={SOCKET_NAME}");

    let mut app = App {
        renderer: None,
        surface: None,
        window: None,
        gpu,
        display,
        stack: build_stack(),
        cursor: (0.0, 0.0),
        drag: None,
        wayland,
        mapped: Vec::new(),
        started: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

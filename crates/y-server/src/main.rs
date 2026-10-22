mod wayland;
mod window;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle};
use smithay::backend::input::{ButtonState, KeyState};
use smithay::reexports::wayland_server::backend::ObjectId;
use std::env;
use std::time::{Duration, Instant};
use wayland::WaylandServer;
use window::WindowStack;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{ModifiersState, PhysicalKey};
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::window::{CursorIcon, Window, WindowId};
use y_gpu::{GpuContext, Renderer, Surface};

const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const SOCKET_NAME: &str = "wayland-y";
const RESIZE_MARGIN: f32 = 8.0;

struct App {
    renderer: Option<Renderer>,
    surface: Option<Surface>,
    window: Option<Window>,
    gpu: GpuContext,
    display: RawDisplayHandle,
    stack: WindowStack,
    cursor: (f32, f32),
    drag: Option<(u32, f32, f32)>,
    resize: Option<(u32, f32, f32, f32, f32)>,
    modifiers: ModifiersState,
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

    fn current_surface(&self) -> (Option<ObjectId>, (f64, f64)) {
        if let Some(window_id) = self.stack.hit_test(self.cursor.0, self.cursor.1) {
            if let Some((id, _)) = self.mapped.iter().find(|(_, wid)| *wid == window_id) {
                if let Some((wx, wy)) = self.stack.position(window_id) {
                    return (Some(id.clone()), (wx as f64, wy as f64));
                }
            }
        }
        (None, (0.0, 0.0))
    }

    fn update_pointer(&mut self) {
        let (surface_id, local) = self.current_surface();
        self.wayland.pointer_motion(
            surface_id,
            local,
            (self.cursor.0 as f64, self.cursor.1 as f64),
        );
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
            if matches!(self.resize, Some((id, _, _, _, _)) if id == window_id) {
                self.resize = None;
            }
            println!("Y.Org: Wayland toplevel closed, window {window_id} removed");
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
            println!("Y.Org: new Wayland toplevel mapped as window {id}");
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
            // Atualiza o ponteiro para garantir que o foco seja reavaliado
            // se uma nova janela apareceu sob o cursor.
            self.update_pointer();
        }
    }

    fn edge_hit(&self, x: f32, y: f32) -> Option<(u32, u8)> {
        for w in self.stack.windows.iter().rev() {
            if x < w.x || x > w.x + w.width || y < w.y || y > w.y + w.height {
                continue;
            }
            let left = (x - w.x).abs() < RESIZE_MARGIN;
            let right = (w.x + w.width - x).abs() < RESIZE_MARGIN;
            let top = (y - w.y).abs() < RESIZE_MARGIN;
            let bottom = (w.y + w.height - y).abs() < RESIZE_MARGIN;

            let mut edges = 0u8;
            if left {
                edges |= 1;
            }
            if right {
                edges |= 2;
            }
            if top {
                edges |= 4;
            }
            if bottom {
                edges |= 8;
            }

            return if edges != 0 {
                Some((w.id, edges))
            } else {
                None
            };
        }
        None
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window = event_loop
            .create_window(Window::default_attributes().with_title("Y.Org"))
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
        let renderer = self
            .gpu
            .create_renderer(&surface, (size.width, size.height), gpu_index)
            .expect("failed to create renderer");

        println!(
            "Y.Org: renderer ready | GPU: {} | format {:?} | {}x{} | {} swapchain images",
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
                println!("Y.Org: shutting down...");
                event_loop.exit();
            }
            WindowEvent::Resized(_) => {
                self.resize_to_window();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);

                if let Some((_id, edges)) = self.edge_hit(self.cursor.0, self.cursor.1) {
                    if let Some(window) = &self.window {
                        let icon = match edges {
                            1 | 2 => CursorIcon::EwResize,
                            4 | 8 => CursorIcon::NsResize,
                            3 => CursorIcon::EwResize,
                            5 | 10 => CursorIcon::NwseResize,
                            6 | 9 => CursorIcon::NeswResize,
                            12 => CursorIcon::NsResize,
                            _ => CursorIcon::Default,
                        };
                        window.set_cursor(icon);
                    }
                } else if let Some(window) = &self.window {
                    window.set_cursor(CursorIcon::Default);
                }

                if let Some((id, dx, dy)) = self.drag {
                    self.stack
                        .move_to(id, self.cursor.0 - dx, self.cursor.1 - dy);
                    self.update_pointer();
                    self.upload_scene();
                } else if let Some((id, start_x, start_y, start_w, start_h)) = self.resize {
                    let new_w = (start_w + (self.cursor.0 - start_x)).max(100.0);
                    let new_h = (start_h + (self.cursor.1 - start_y)).max(100.0);
                    self.stack.set_body_size(id, new_w as u32, new_h as u32);
                    if let Some((surface_id, _)) = self.mapped.iter().find(|(_, wid)| *wid == id) {
                        self.wayland.resize_toplevel(
                            surface_id.clone(),
                            new_w as i32,
                            new_h as i32,
                        );
                    }
                    self.upload_scene();
                } else {
                    self.update_pointer();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                let btn_state = if state == ElementState::Pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };

                if state == ElementState::Pressed {
                    if let Some((id, _edges)) = self.edge_hit(self.cursor.0, self.cursor.1) {
                        if let Some(w) = self.stack.windows.iter().find(|w| w.id == id) {
                            self.resize =
                                Some((id, self.cursor.0, self.cursor.1, w.width, w.height));
                        }
                    } else if self.modifiers.shift_key() {
                        if let Some(id) = self.stack.hit_test(self.cursor.0, self.cursor.1) {
                            self.stack.raise(id);
                            if let Some((wx, wy)) = self.stack.position(id) {
                                self.drag = Some((id, self.cursor.0 - wx, self.cursor.1 - wy));
                            }
                            if let Some((surface_id, _)) =
                                self.mapped.iter().find(|(_, wid)| *wid == id)
                            {
                                self.wayland.set_focus(surface_id.clone());
                            }
                            self.upload_scene();
                        }
                    } else if let Some(id) = self.stack.hit_test(self.cursor.0, self.cursor.1) {
                        self.stack.raise(id);

                        if let Some((surface_id, _)) =
                            self.mapped.iter().find(|(_, wid)| *wid == id)
                        {
                            self.wayland.set_focus(surface_id.clone());
                        }

                        // Atualiza o ponteiro (motion) para garantir que o foco
                        // esteja correto antes de enviar o botão.
                        self.update_pointer();
                        self.wayland.pointer_button(0x110, btn_state);
                        self.upload_scene();
                    }
                } else {
                    self.drag = None;
                    self.resize = None;
                    self.update_pointer();
                    self.wayland.pointer_button(0x110, btn_state);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(scancode) = code.to_scancode() {
                        let key_state = if event.state == ElementState::Pressed {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        };
                        self.wayland.key_input(scancode, key_state);
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
    unsafe {
        env::set_var("XDG_CURRENT_DESKTOP", "Y.Org");
        env::set_var("XDG_SESSION_TYPE", "wayland");
    }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let display = event_loop.display_handle()?.as_raw();
    let gpu = GpuContext::new(display)?;

    let gpus = gpu.list_gpus()?;
    println!("Y.Org: {} GPU(s) found", gpus.len());
    for (i, g) in gpus.iter().enumerate() {
        let (a, b, c) = g.vulkan_version;
        println!(
            "  [{}] {} ({:?}) - Vulkan {}.{}.{}",
            i, g.name, g.device_type, a, b, c
        );
    }

    let wayland = WaylandServer::new(SOCKET_NAME)?;
    println!("Y.Org: Wayland socket ready, use WAYLAND_DISPLAY={SOCKET_NAME}");

    let mut app = App {
        renderer: None,
        surface: None,
        window: None,
        gpu,
        display,
        stack: WindowStack::default(),
        cursor: (0.0, 0.0),
        drag: None,
        resize: None,
        modifiers: ModifiersState::empty(),
        wayland,
        mapped: Vec::new(),
        started: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

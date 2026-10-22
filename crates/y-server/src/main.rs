mod wayland;
mod window;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle};
use smithay::backend::input::{ButtonState, KeyState};
use smithay::reexports::wayland_server::backend::ObjectId;
use std::env;
use std::time::{Duration, Instant};
use wayland::WaylandServer;
use window::{Decorations, Restore, WindowStack};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::window::{CursorIcon, Window, WindowId};
use y_gpu::{GpuContext, Renderer, Surface};

const BACKGROUND: [f32; 4] = [0.08, 0.05, 0.18, 1.0];
const SOCKET_NAME: &str = "wayland-y";
const RESIZE_MARGIN: f32 = 6.0;
const MIN_OUTER_W: f32 = 120.0;
const MIN_OUTER_H: f32 = 100.0;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

#[derive(Clone, Copy)]
struct ResizeState {
    window_id: u32,
    start_cursor: (f32, f32),
    start_outer: (f32, f32, f32, f32),
    edges: u8,
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
    resize: Option<ResizeState>,
    modifiers: ModifiersState,
    wayland: WaylandServer,
    mapped: Vec<(ObjectId, u32)>,
    popup_windows: Vec<(ObjectId, u32)>,
    focused: Option<u32>,
    started: Instant,
}

impl App {
    fn screen_size(&self) -> (f32, f32) {
        self.window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width as f32, s.height as f32)
            })
            .unwrap_or((800.0, 600.0))
    }

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
        let screen = self.screen_size();
        if let Some(renderer) = &mut self.renderer {
            let (vertices, draws) = self.stack.scene(screen);
            renderer
                .set_scene(&vertices, &draws)
                .expect("failed to upload scene");
        }
    }

    fn window_for_surface(&self, surface_id: &ObjectId) -> Option<u32> {
        self.mapped
            .iter()
            .chain(self.popup_windows.iter())
            .find(|(known, _)| known == surface_id)
            .map(|(_, id)| *id)
    }

    fn surface_for_window(&self, window_id: u32) -> Option<ObjectId> {
        self.mapped
            .iter()
            .chain(self.popup_windows.iter())
            .find(|(_, id)| *id == window_id)
            .map(|(surface_id, _)| surface_id.clone())
    }

    fn is_popup_window(&self, window_id: u32) -> bool {
        self.popup_windows.iter().any(|(_, id)| *id == window_id)
    }

    fn set_focus(&mut self, id: Option<u32>) {
        self.focused = id;
        self.stack.set_focused(id);
    }

    fn outer_of(&self, id: u32) -> Option<(f32, f32, f32, f32)> {
        let w = self.stack.window(id)?;
        let (oy, oh) = if w.decorations == Decorations::Server {
            (w.y - window::TITLE_BAR_HEIGHT, w.height + window::TITLE_BAR_HEIGHT)
        } else {
            (w.y, w.height)
        };
        Some((w.x, oy, w.width, oh))
    }

    fn current_surface(&self) -> (Option<ObjectId>, (f64, f64)) {
        if let Some((window_id, on_bar)) = self.stack.hit_test(self.cursor.0, self.cursor.1) {
            if on_bar {
                return (None, (0.0, 0.0));
            }
            if let Some(surface_id) = self.surface_for_window(window_id) {
                if let Some((wx, wy)) = self.stack.position(window_id) {
                    return (Some(surface_id), (wx as f64, wy as f64));
                }
            }
        }
        (None, (0.0, 0.0))
    }

    fn update_pointer(&mut self) {
        let (surface_id, origin) = self.current_surface();
        self.wayland.pointer_motion(
            surface_id,
            origin,
            (self.cursor.0 as f64, self.cursor.1 as f64),
        );
    }

    fn minimize_window(&mut self, id: u32) {
        if let Some(w) = self.stack.window_mut(id) {
            if w.minimized {
                return;
            }
            w.minimized = true;
        }
        if self.focused == Some(id) {
            self.set_focus(None);
            self.wayland.clear_focus();
        }
        self.upload_scene();
    }

    fn restore_window(&mut self, id: u32) {
        if let Some(w) = self.stack.window_mut(id) {
            w.minimized = false;
        }
        self.stack.raise(id);
        if let Some(surface_id) = self.surface_for_window(id) {
            self.wayland.set_focus(surface_id);
        }
        self.set_focus(Some(id));
        self.upload_scene();
    }

    fn toggle_maximize(&mut self, id: u32) {
        let screen = self.screen_size();

        let (maximized, restore) = match self.stack.window(id) {
            Some(w) => (w.maximized, w.restore),
            None => return,
        };

        if maximized {
            if let Some(r) = restore {
                if let Some(w) = self.stack.window_mut(id) {
                    w.x = r.x;
                    w.y = r.y;
                    w.width = r.width as f32;
                    w.height = r.height as f32;
                    w.maximized = false;
                    w.restore = None;
                }
                if let Some(surface_id) = self.surface_for_window(id) {
                    self.wayland.resize_toplevel(surface_id, r.width, r.height);
                }
            }
        } else {
            let (cx, cy, cw, ch) = match self.stack.window(id) {
                Some(w) => (w.x, w.y, w.width as i32, w.height as i32),
                None => return,
            };
            if let Some(w) = self.stack.window_mut(id) {
                w.restore = Some(Restore {
                    x: cx,
                    y: cy,
                    width: cw,
                    height: ch,
                });
                w.x = 0.0;
                w.y = window::TITLE_BAR_HEIGHT;
                w.width = screen.0;
                w.height = screen.1 - window::TITLE_BAR_HEIGHT;
                w.maximized = true;
            }
            if let Some(surface_id) = self.surface_for_window(id) {
                self.wayland.resize_toplevel(
                    surface_id,
                    screen.0 as i32,
                    (screen.1 - window::TITLE_BAR_HEIGHT) as i32,
                );
            }
        }
        self.upload_scene();
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
            if matches!(self.resize, Some(rs) if rs.window_id == window_id) {
                self.resize = None;
            }
            if self.focused == Some(window_id) {
                self.set_focus(None);
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
                80.0 + 30.0 * n + window::TITLE_BAR_HEIGHT,
                400.0,
                300.0,
                [0.45, 0.45, 0.5],
            );
            self.stack.raise(id);
            self.mapped.push((surface_id, id));
            if self.focused.is_none() {
                self.set_focus(Some(id));
            }
            println!("Y.Org: new Wayland toplevel mapped as window {id}");
            changed = true;
        }

        for (surface_id, window_id) in &self.mapped {
            let csd = self.wayland.has_csd(surface_id.clone());
            self.stack.set_csd(*window_id, csd);
        }

        let popups = self.wayland.popup_infos();

        let mut popup_gone = Vec::new();
        self.popup_windows.retain(|(surface_id, window_id)| {
            let keep = popups.iter().any(|p| p.id == *surface_id);
            if !keep {
                popup_gone.push(*window_id);
            }
            keep
        });
        for window_id in popup_gone {
            self.stack.remove(window_id);
            changed = true;
        }

        for info in &popups {
            let Some(parent_window) = self.window_for_surface(&info.parent) else {
                continue;
            };
            let Some((parent_x, parent_y)) = self.stack.position(parent_window) else {
                continue;
            };
            let x = parent_x + info.offset.0 as f32;
            let y = parent_y + info.offset.1 as f32;

            let existing = self
                .popup_windows
                .iter()
                .find(|(known, _)| *known == info.id)
                .map(|(_, id)| *id);

            match existing {
                Some(window_id) => {
                    if self.stack.position(window_id) != Some((x, y)) {
                        self.stack.move_to(window_id, x, y);
                        changed = true;
                    }
                }
                None => {
                    let window_id = self.stack.add(x, y, 0.0, 0.0, [0.0, 0.0, 0.0]);
                    self.stack.set_csd(window_id, true);
                    self.stack.raise(window_id);
                    self.popup_windows.push((info.id.clone(), window_id));
                    println!("Y.Org: new Wayland popup mapped as window {window_id}");
                    changed = true;
                }
            }
        }

        for (surface_id, pixels) in self.wayland.take_updates() {
            let Some(window_id) = self.window_for_surface(&surface_id) else {
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
            self.update_pointer();
        }
    }

    fn edge_hit(&self, x: f32, y: f32) -> Option<(u32, u8)> {
        for w in self.stack.windows.iter().rev() {
            if w.minimized || self.is_popup_window(w.id) {
                continue;
            }
            let Some((ox, oy, ow, oh)) = self.outer_of(w.id) else {
                continue;
            };
            if x < ox || x > ox + ow || y < oy || y > oy + oh {
                continue;
            }
            let left = (x - ox).abs() < RESIZE_MARGIN;
            let right = (ox + ow - x).abs() < RESIZE_MARGIN;
            let top_edge = (y - oy).abs() < RESIZE_MARGIN;
            let bottom = (oy + oh - y).abs() < RESIZE_MARGIN;

            let mut edges = 0u8;
            if left { edges |= 1; }
            if right { edges |= 2; }
            if top_edge { edges |= 4; }
            if bottom { edges |= 8; }
            return if edges != 0 { Some((w.id, edges)) } else { None };
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
            WindowEvent::Resized(_) => self.resize_to_window(),
            WindowEvent::ModifiersChanged(mods) => self.modifiers = mods.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);

                if let Some((_id, edges)) = self.edge_hit(self.cursor.0, self.cursor.1) {
                    if let Some(window) = &self.window {
                        let icon = match edges {
                            1 | 2 => CursorIcon::EwResize,
                            4 | 8 => CursorIcon::NsResize,
                            3 | 6 | 9 | 12 => CursorIcon::NwseResize,
                            5 | 10 => CursorIcon::NeswResize,
                            _ => CursorIcon::Default,
                        };
                        window.set_cursor(icon);
                    }
                } else if let Some(window) = &self.window {
                    window.set_cursor(CursorIcon::Default);
                }

                if let Some((id, dx, dy)) = self.drag {
                    let nx = self.cursor.0 - dx;
                    let ny_outer = self.cursor.1 - dy;
                    let ny = if let Some(w) = self.stack.window(id) {
                        if w.decorations == Decorations::Server {
                            ny_outer + window::TITLE_BAR_HEIGHT
                        } else {
                            ny_outer
                        }
                    } else {
                        ny_outer
                    };
                    self.stack.move_to(id, nx, ny);
                    self.update_pointer();
                    self.upload_scene();
                } else if let Some(rs) = self.resize {
                    let dx = self.cursor.0 - rs.start_cursor.0;
                    let dy = self.cursor.1 - rs.start_cursor.1;

                    let (mut ox, mut oy, mut ow, mut oh) = rs.start_outer;

                    if rs.edges & 1 != 0 {
                        ox += dx;
                        ow -= dx;
                    }
                    if rs.edges & 2 != 0 {
                        ow += dx;
                    }
                    if rs.edges & 4 != 0 {
                        oy += dy;
                        oh -= dy;
                    }
                    if rs.edges & 8 != 0 {
                        oh += dy;
                    }

                    if ow < MIN_OUTER_W {
                        if rs.edges & 1 != 0 {
                            ox = rs.start_outer.0 + rs.start_outer.2 - MIN_OUTER_W;
                        }
                        ow = MIN_OUTER_W;
                    }
                    if oh < MIN_OUTER_H {
                        if rs.edges & 4 != 0 {
                            oy = rs.start_outer.1 + rs.start_outer.3 - MIN_OUTER_H;
                        }
                        oh = MIN_OUTER_H;
                    }

                    let (cy, ch) = match self.stack.window(rs.window_id) {
                        Some(w) if w.decorations == Decorations::Server => {
                            (oy + window::TITLE_BAR_HEIGHT, oh - window::TITLE_BAR_HEIGHT)
                        }
                        _ => (oy, oh),
                    };

                    self.stack.set_geometry(rs.window_id, ox, cy, ow, ch);
                    if let Some((surface_id, _)) =
                        self.mapped.iter().find(|(_, wid)| *wid == rs.window_id)
                    {
                        self.wayland
                            .resize_toplevel(surface_id.clone(), ow as i32, ch as i32);
                    }
                    self.upload_scene();
                } else {
                    self.update_pointer();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let code = match button {
                    MouseButton::Left => BTN_LEFT,
                    MouseButton::Right => BTN_RIGHT,
                    MouseButton::Middle => BTN_MIDDLE,
                    _ => return,
                };
                let is_left = button == MouseButton::Left;
                let btn_state = if state == ElementState::Pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };

                if state == ElementState::Pressed {
                    if is_left {
                        if let Some(id) =
                            self.stack.hit_close_button(self.cursor.0, self.cursor.1)
                        {
                            if let Some(surface_id) = self.surface_for_window(id) {
                                self.wayland.close_toplevel(surface_id);
                            }
                            return;
                        }
                    }

                    let screen = self.screen_size();
                    if let Some(docked_id) =
                        self.stack.dock_hit(self.cursor.0, self.cursor.1, screen)
                    {
                        self.restore_window(docked_id);
                        return;
                    }

                    if is_left {
                        if let Some((id, edges)) = self.edge_hit(self.cursor.0, self.cursor.1) {
                            if let Some(outer) = self.outer_of(id) {
                                self.resize = Some(ResizeState {
                                    window_id: id,
                                    start_cursor: (self.cursor.0, self.cursor.1),
                                    start_outer: outer,
                                    edges,
                                });
                                return;
                            }
                        }
                    }

                    let hit = self.stack.hit_test(self.cursor.0, self.cursor.1);

                    if is_left {
                        if let Some((id, true)) = hit {
                            self.stack.raise(id);
                            if let Some(surface_id) = self.surface_for_window(id) {
                                self.wayland.set_focus(surface_id);
                            }
                            self.set_focus(Some(id));
                            if let Some((ox, oy, _, _)) = self.outer_of(id) {
                                self.drag =
                                    Some((id, self.cursor.0 - ox, self.cursor.1 - oy));
                            }
                            self.upload_scene();
                            return;
                        }
                    }

                    let on_popup = hit.is_some_and(|(id, _)| self.is_popup_window(id));

                    if !on_popup && self.wayland.has_popups() {
                        self.wayland.dismiss_popups();
                    }

                    if is_left && !on_popup && self.modifiers.shift_key() {
                        if let Some((id, _)) = hit {
                            self.stack.raise(id);
                            if let Some((ox, oy, _, _)) = self.outer_of(id) {
                                self.drag =
                                    Some((id, self.cursor.0 - ox, self.cursor.1 - oy));
                            }
                            if let Some(surface_id) = self.surface_for_window(id) {
                                self.wayland.set_focus(surface_id);
                            }
                            self.set_focus(Some(id));
                            self.upload_scene();
                        }
                    } else if let Some((id, _)) = hit {
                        if !on_popup {
                            self.stack.raise(id);
                            if let Some(surface_id) = self.surface_for_window(id) {
                                self.wayland.set_focus(surface_id);
                            }
                            self.set_focus(Some(id));
                        }
                        self.update_pointer();
                        self.wayland.pointer_button(code, btn_state);
                        self.upload_scene();
                    }
                } else {
                    if is_left {
                        self.drag = None;
                        self.resize = None;
                    }
                    self.update_pointer();
                    self.wayland.pointer_button(code, btn_state);
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

                        if key_state == KeyState::Pressed && self.modifiers.super_key() {
                            match code {
                                KeyCode::KeyM => {
                                    if let Some(id) = self.focused {
                                        self.minimize_window(id);
                                    }
                                    return;
                                }
                                KeyCode::ArrowUp => {
                                    if let Some(id) = self.focused {
                                        self.toggle_maximize(id);
                                    }
                                    return;
                                }
                                KeyCode::ArrowDown => {
                                    if let Some(id) = self.focused {
                                        let maximized = self
                                            .stack
                                            .window(id)
                                            .map(|w| w.maximized)
                                            .unwrap_or(false);
                                        if maximized {
                                            self.toggle_maximize(id);
                                        } else {
                                            self.minimize_window(id);
                                        }
                                    }
                                    return;
                                }
                                KeyCode::KeyQ => {
                                    if let Some(id) = self.focused {
                                        if let Some(surface_id) = self.surface_for_window(id) {
                                            self.wayland.close_toplevel(surface_id);
                                        }
                                    }
                                    return;
                                }
                                _ => {}
                            }
                        }

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
        popup_windows: Vec::new(),
        focused: None,
        started: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

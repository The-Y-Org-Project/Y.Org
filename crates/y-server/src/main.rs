mod window;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle};
use window::WindowStack;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};
use y_gpu::{GpuContext, Renderer, Surface};

const BACKGROUND: [f32; 4] = [0.08, 0.05, 0.18, 1.0];

fn build_stack() -> WindowStack {
    let mut stack = WindowStack::default();
    let blue = stack.add(60.0, 60.0, 360.0, 260.0, [0.2, 0.4, 0.9]);
    stack.add(260.0, 160.0, 320.0, 240.0, [0.2, 0.75, 0.4]);
    stack.add(440.0, 80.0, 280.0, 220.0, [0.95, 0.6, 0.15]);
    stack.raise(blue);
    stack
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
            renderer
                .set_vertices(&self.stack.vertices())
                .expect("failed to upload vertices");
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
        let renderer = self
            .gpu
            .create_renderer(&surface, (size.width, size.height), gpu_index)
            .expect("failed to create renderer");

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
                if let Some((id, dx, dy)) = self.drag {
                    self.stack
                        .move_to(id, self.cursor.0 - dx, self.cursor.1 - dy);
                    self.upload_scene();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                match state {
                    ElementState::Pressed => {
                        if let Some((id, on_title)) =
                            self.stack.hit_test(self.cursor.0, self.cursor.1)
                        {
                            self.stack.raise(id);
                            if on_title {
                                if let Some((wx, wy)) = self.stack.position(id) {
                                    self.drag = Some((id, self.cursor.0 - wx, self.cursor.1 - wy));
                                }
                            }
                            self.upload_scene();
                        }
                    }
                    ElementState::Released => {
                        self.drag = None;
                    }
                }
            }
            WindowEvent::RedrawRequested => {
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

    let mut app = App {
        renderer: None,
        surface: None,
        window: None,
        gpu,
        display,
        stack: build_stack(),
        cursor: (0.0, 0.0),
        drag: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

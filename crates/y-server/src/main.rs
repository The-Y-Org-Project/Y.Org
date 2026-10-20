use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};
use y_gpu::{GpuContext, Renderer, Surface, Vertex};

const BACKGROUND: [f32; 4] = [0.08, 0.05, 0.18, 1.0];

fn scene() -> Vec<Vertex> {
    let red = [1.0, 0.0, 0.0];
    let green = [0.0, 1.0, 0.0];
    let blue = [0.0, 0.0, 1.0];
    let yellow = [1.0, 0.9, 0.0];
    let magenta = [1.0, 0.0, 1.0];
    let cyan = [0.0, 1.0, 1.0];
    let white = [1.0, 1.0, 1.0];

    vec![
        Vertex::new([-0.5, -0.6], red),
        Vertex::new([-0.05, 0.6], green),
        Vertex::new([-0.95, 0.6], blue),
        Vertex::new([0.1, -0.6], yellow),
        Vertex::new([0.9, -0.6], magenta),
        Vertex::new([0.9, 0.6], cyan),
        Vertex::new([0.1, -0.6], yellow),
        Vertex::new([0.9, 0.6], cyan),
        Vertex::new([0.1, 0.6], white),
    ]
}

struct App {
    renderer: Option<Renderer>,
    surface: Option<Surface>,
    window: Option<Window>,
    gpu: GpuContext,
    display: RawDisplayHandle,
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

        renderer
            .set_vertices(&scene())
            .expect("failed to upload vertices");

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
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

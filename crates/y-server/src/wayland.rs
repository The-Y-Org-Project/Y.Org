use smithay::{
    backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state},
    delegate_compositor, delegate_seat, delegate_shm, delegate_xdg_shell,
    input::{Seat, SeatHandler, SeatState},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, Display, ListeningSocket, Resource,
            backend::{ClientData, ClientId, DisconnectReason, ObjectId},
            protocol::{
                wl_buffer::{self, WlBuffer},
                wl_seat,
                wl_shm::Format,
                wl_surface::WlSurface,
            },
        },
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, with_surface_tree_downward,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{BufferData, ShmHandler, ShmState, with_buffer_contents},
    },
};
use std::error::Error;
use std::sync::Arc;

pub struct SurfacePixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct WaylandState {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    _seat: Seat<Self>,
    updates: Vec<(ObjectId, SurfacePixels)>,
}

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {
        println!("Y: Wayland client connected");
    }

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        println!("Y: Wayland client disconnected");
    }
}

fn read_shm_pixels(buffer: &WlBuffer) -> Option<SurfacePixels> {
    with_buffer_contents(buffer, |ptr, len, data: BufferData| {
        let has_alpha = match data.format {
            Format::Argb8888 => true,
            Format::Xrgb8888 => false,
            _ => return None,
        };
        if data.width <= 0 || data.height <= 0 || data.stride <= 0 || data.offset < 0 {
            return None;
        }

        let width = data.width as usize;
        let height = data.height as usize;
        let stride = data.stride as usize;
        let offset = data.offset as usize;
        let row_bytes = width * 4;
        if row_bytes > stride || offset + (height - 1) * stride + row_bytes > len {
            return None;
        }

        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            let start = offset + y * stride;
            for px in bytes[start..start + row_bytes].chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], if has_alpha { px[3] } else { 255 }]);
            }
        }

        Some(SurfacePixels {
            width: width as u32,
            height: height as u32,
            rgba,
        })
    })
    .ok()
    .flatten()
}

impl BufferHandler for WaylandState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl CompositorHandler for WaylandState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        let pixels = with_renderer_surface_state(surface, |state| {
            state.buffer().and_then(|buffer| read_shm_pixels(buffer))
        })
        .flatten();

        if let Some(pixels) = pixels {
            let id = surface.id();
            self.updates.retain(|(known, _)| *known != id);
            self.updates.push((id, pixels));
        }
    }
}

impl ShmHandler for WaylandState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for WaylandState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

impl XdgShellHandler for WaylandState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
        });
        surface.send_configure();
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

delegate_compositor!(WaylandState);
delegate_shm!(WaylandState);
delegate_seat!(WaylandState);
delegate_xdg_shell!(WaylandState);

pub struct WaylandServer {
    display: Display<WaylandState>,
    state: WaylandState,
    listener: ListeningSocket,
    _clients: Vec<Client>,
}

impl WaylandServer {
    pub fn new(socket_name: &str) -> Result<Self, Box<dyn Error>> {
        let display: Display<WaylandState> = Display::new()?;
        let dh = display.handle();

        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(&dh, "y-seat");

        let state = WaylandState {
            compositor_state: CompositorState::new::<WaylandState>(&dh),
            xdg_shell_state: XdgShellState::new::<WaylandState>(&dh),
            shm_state: ShmState::new::<WaylandState>(&dh, vec![]),
            seat_state,
            _seat: seat,
            updates: Vec::new(),
        };

        let listener = ListeningSocket::bind(socket_name)?;

        Ok(Self {
            display,
            state,
            listener,
            _clients: Vec::new(),
        })
    }

    pub fn dispatch(&mut self) -> Result<(), Box<dyn Error>> {
        while let Some(stream) = self.listener.accept()? {
            let client = self
                .display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))?;
            self._clients.push(client);
        }
        self.display.dispatch_clients(&mut self.state)?;
        self.display.flush_clients()?;
        Ok(())
    }

    pub fn toplevel_ids(&self) -> Vec<ObjectId> {
        self.state
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .map(|surface| surface.wl_surface().id())
            .collect()
    }

    pub fn take_updates(&mut self) -> Vec<(ObjectId, SurfacePixels)> {
        std::mem::take(&mut self.state.updates)
    }

    pub fn send_frames(&self, time: u32) {
        for surface in self.state.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(surface.wl_surface(), time);
        }
    }
}

fn send_frames_surface_tree(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

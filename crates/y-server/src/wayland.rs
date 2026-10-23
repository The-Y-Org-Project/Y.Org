use smithay::{
    backend::{
        input::{Axis, AxisSource, ButtonState, KeyState},
        renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state},
    },
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_decoration, delegate_xdg_shell,
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, xkb},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1,
            shell::server::xdg_toplevel,
        },
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
    utils::{Logical, Point, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, with_states, with_surface_tree_downward,
        },
        output::OutputHandler,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
            XdgShellState,
            decoration::{XdgDecorationHandler, XdgDecorationState},
        },
        shm::{BufferData, ShmHandler, ShmState, with_buffer_contents},
    },
};
use std::collections::HashSet;
use std::error::Error;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Instant;

pub const EDGE_LEFT: u8 = 1;
pub const EDGE_RIGHT: u8 = 2;
pub const EDGE_TOP: u8 = 4;
pub const EDGE_BOTTOM: u8 = 8;

static SERIAL: AtomicU32 = AtomicU32::new(1);

fn next_serial() -> Serial {
    Serial::from(SERIAL.fetch_add(1, Ordering::SeqCst))
}

pub struct SurfacePixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct PopupInfo {
    pub id: ObjectId,
    pub parent: ObjectId,
    pub offset: (i32, i32),
}

pub enum ClientRequest {
    Move { surface: ObjectId },
    Resize { surface: ObjectId, edges: u8 },
    Maximize { surface: ObjectId, on: bool },
    Minimize { surface: ObjectId },
}

struct WaylandState {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    #[allow(dead_code)]
    xdg_decoration_state: XdgDecorationState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    output: Output,
    pub seat: Seat<Self>,
    updates: Vec<(ObjectId, SurfacePixels)>,
    popups: Vec<PopupSurface>,
    requests: Vec<ClientRequest>,
    cursor_name: String,
    csd_surfaces: HashSet<ObjectId>,
}

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {
        println!("Y.Org: Wayland client connected");
    }

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        println!("Y.Org: Wayland client disconnected");
    }
}

fn window_geometry(surface: &WlSurface) -> Option<(i32, i32, i32, i32)> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
            .map(|g| (g.loc.x, g.loc.y, g.size.w, g.size.h))
    })
}

fn geometry_origin(surface: &WlSurface) -> (i32, i32) {
    window_geometry(surface)
        .map(|g| (g.0, g.1))
        .unwrap_or((0, 0))
}

fn edges_from(edge: xdg_toplevel::ResizeEdge) -> u8 {
    use xdg_toplevel::ResizeEdge as Edge;
    match edge {
        Edge::Top => EDGE_TOP,
        Edge::Bottom => EDGE_BOTTOM,
        Edge::Left => EDGE_LEFT,
        Edge::Right => EDGE_RIGHT,
        Edge::TopLeft => EDGE_TOP | EDGE_LEFT,
        Edge::TopRight => EDGE_TOP | EDGE_RIGHT,
        Edge::BottomLeft => EDGE_BOTTOM | EDGE_LEFT,
        Edge::BottomRight => EDGE_BOTTOM | EDGE_RIGHT,
        _ => 0,
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
            rgba.extend_from_slice(&bytes[start..start + row_bytes]);
        }

        if !has_alpha {
            for px in rgba.chunks_exact_mut(4) {
                px[3] = 255;
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

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_name = match image {
            CursorImageStatus::Hidden => "none".to_string(),
            CursorImageStatus::Named(icon) => icon.name().to_string(),
            #[allow(unreachable_patterns)]
            _ => "default".to_string(),
        };
    }
}

impl OutputHandler for WaylandState {}

impl SelectionHandler for WaylandState {
    type SelectionUserData = ();
}

impl ClientDndGrabHandler for WaylandState {}

impl ServerDndGrabHandler for WaylandState {}

impl DataDeviceHandler for WaylandState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl XdgDecorationHandler for WaylandState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        mode: zxdg_toplevel_decoration_v1::Mode,
    ) {
        let id = toplevel.wl_surface().id();
        match mode {
            zxdg_toplevel_decoration_v1::Mode::ClientSide => {
                self.csd_surfaces.insert(id);
            }
            zxdg_toplevel_decoration_v1::Mode::ServerSide => {
                self.csd_surfaces.remove(&id);
            }
            _ => {}
        }
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        let id = toplevel.wl_surface().id();
        self.csd_surfaces.remove(&id);
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        });
        toplevel.send_configure();
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

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
        let _ = surface.send_configure();
        self.popups.push(surface);
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        self.requests.push(ClientRequest::Move {
            surface: surface.wl_surface().id(),
        });
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        self.requests.push(ClientRequest::Resize {
            surface: surface.wl_surface().id(),
            edges: edges_from(edges),
        });
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.requests.push(ClientRequest::Maximize {
            surface: surface.wl_surface().id(),
            on: true,
        });
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.requests.push(ClientRequest::Maximize {
            surface: surface.wl_surface().id(),
            on: false,
        });
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        self.requests.push(ClientRequest::Minimize {
            surface: surface.wl_surface().id(),
        });
    }

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
delegate_xdg_decoration!(WaylandState);
delegate_output!(WaylandState);
delegate_data_device!(WaylandState);

pub struct WaylandServer {
    display: Display<WaylandState>,
    state: WaylandState,
    listener: ListeningSocket,
    _clients: Vec<Client>,
    started: Instant,
}

impl WaylandServer {
    pub fn new(socket_name: &str) -> Result<Self, Box<dyn Error>> {
        let display: Display<WaylandState> = Display::new()?;
        let dh = display.handle();

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "y-seat");

        seat.add_keyboard(Default::default(), 200, 25)
            .expect("Failed to add keyboard");
        seat.add_pointer();

        let data_device_state = DataDeviceState::new::<WaylandState>(&dh);

        let output = Output::new(
            "Y.Org Display".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Y.Org".into(),
                model: "Y.Compositor".into(),
            },
        );
        let _global = output.create_global::<WaylandState>(&dh);
        let mode = Mode {
            size: (800, 600).into(),
            refresh: 60000,
        };
        output.change_current_state(Some(mode), None, None, None);
        output.set_preferred(mode);

        let state = WaylandState {
            compositor_state: CompositorState::new::<WaylandState>(&dh),
            xdg_shell_state: XdgShellState::new::<WaylandState>(&dh),
            xdg_decoration_state: XdgDecorationState::new::<WaylandState>(&dh),
            shm_state: ShmState::new::<WaylandState>(&dh, vec![]),
            seat_state,
            data_device_state,
            output,
            seat,
            updates: Vec::new(),
            popups: Vec::new(),
            requests: Vec::new(),
            cursor_name: "default".to_string(),
            csd_surfaces: HashSet::new(),
        };

        let listener = ListeningSocket::bind(socket_name)?;

        Ok(Self {
            display,
            state,
            listener,
            _clients: Vec::new(),
            started: Instant::now(),
        })
    }

    pub fn uses_csd(&self, id: &ObjectId) -> bool {
        self.state.csd_surfaces.contains(id)
    }

    pub fn mark_csd(&mut self, id: &ObjectId) {
        self.state.csd_surfaces.insert(id.clone());
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

    fn now_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    pub fn set_output_size(&mut self, width: i32, height: i32) {
        let mode = Mode {
            size: (width, height).into(),
            refresh: 60000,
        };
        self.state
            .output
            .change_current_state(Some(mode), None, None, None);
        self.state.output.set_preferred(mode);
    }

    pub fn cursor_name(&self) -> String {
        self.state.cursor_name.clone()
    }

    pub fn set_cursor_name(&mut self, name: &str) {
        self.state.cursor_name = name.to_string();
    }

    pub fn toplevel_ids(&self) -> Vec<ObjectId> {
        self.state
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .map(|surface| surface.wl_surface().id())
            .collect()
    }

    pub fn take_requests(&mut self) -> Vec<ClientRequest> {
        std::mem::take(&mut self.state.requests)
    }

    pub fn geometry_of(&self, id: &ObjectId) -> Option<(i32, i32, i32, i32)> {
        self.find_surface(id)
            .and_then(|surface| window_geometry(&surface))
    }

    pub fn configure_toplevel(
        &mut self,
        id: &ObjectId,
        size: Option<(i32, i32)>,
        resizing: Option<bool>,
        maximized: Option<bool>,
    ) {
        let Some(surface) = self
            .state
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .find(|s| s.wl_surface().id() == *id)
            .cloned()
        else {
            return;
        };

        surface.with_pending_state(|state| {
            if let Some((width, height)) = size {
                state.size = Some((width, height).into());
            }
            if let Some(on) = resizing {
                if on {
                    state.states.set(xdg_toplevel::State::Resizing);
                } else {
                    state.states.unset(xdg_toplevel::State::Resizing);
                }
            }
            if let Some(on) = maximized {
                if on {
                    state.states.set(xdg_toplevel::State::Maximized);
                } else {
                    state.states.unset(xdg_toplevel::State::Maximized);
                }
            }
        });
        surface.send_configure();
    }

    pub fn popup_infos(&mut self) -> Vec<PopupInfo> {
        self.state.popups.retain(|p| p.alive());
        self.state
            .popups
            .iter()
            .filter_map(|popup| {
                let parent = popup.get_parent_surface()?;
                let geometry = popup.with_pending_state(|state| state.geometry);
                let (parent_x, parent_y) = geometry_origin(&parent);
                let (own_x, own_y) = geometry_origin(popup.wl_surface());
                Some(PopupInfo {
                    id: popup.wl_surface().id(),
                    parent: parent.id(),
                    offset: (
                        parent_x + geometry.loc.x - own_x,
                        parent_y + geometry.loc.y - own_y,
                    ),
                })
            })
            .collect()
    }

    pub fn has_popups(&self) -> bool {
        self.state.popups.iter().any(|p| p.alive())
    }

    pub fn dismiss_popups(&mut self) {
        for popup in &self.state.popups {
            popup.send_popup_done();
        }
    }

    pub fn take_updates(&mut self) -> Vec<(ObjectId, SurfacePixels)> {
        std::mem::take(&mut self.state.updates)
    }

    pub fn close_toplevel(&self, id: &ObjectId) {
        if let Some(surface) = self
            .state
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .find(|s| s.wl_surface().id() == *id)
        {
            surface.send_close();
        }
    }

    pub fn send_frames(&self, time: u32) {
        for surface in self.state.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(surface.wl_surface(), time);
        }
        for popup in &self.state.popups {
            send_frames_surface_tree(popup.wl_surface(), time);
        }
    }

    fn find_surface(&self, id: &ObjectId) -> Option<WlSurface> {
        self.state
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .map(|s| s.wl_surface().clone())
            .chain(self.state.popups.iter().map(|p| p.wl_surface().clone()))
            .find(|s| s.id() == *id)
    }

    pub fn set_focus(&mut self, id: ObjectId) {
        let surface = self.find_surface(&id);

        if let Some(surface) = surface {
            if let Some(keyboard) = self.state.seat.get_keyboard() {
                let _ = keyboard.set_focus(&mut self.state, Some(surface), next_serial());
            }
        }
    }

    pub fn key_input(&mut self, scancode: u32, key_state: KeyState) {
        let serial = next_serial();
        let time = self.now_ms();
        let keycode = xkb::Keycode::new(scancode + 8);

        if let Some(keyboard) = self.state.seat.get_keyboard() {
            let _ = keyboard.input(
                &mut self.state,
                keycode,
                key_state,
                serial,
                time,
                |_, _, _| FilterResult::<()>::Forward,
            );
        }
    }

    pub fn pointer_motion(
        &mut self,
        surface_id: Option<ObjectId>,
        surface_origin: (f64, f64),
        global_pos: (f64, f64),
    ) {
        let serial = next_serial();
        let time = self.now_ms();
        if let Some(pointer) = self.state.seat.get_pointer() {
            let location = Point::<f64, Logical>::from(global_pos);
            let focus = surface_id.and_then(|id| {
                self.find_surface(&id)
                    .map(|s| (s, Point::<f64, Logical>::from(surface_origin)))
            });

            pointer.motion(
                &mut self.state,
                focus,
                &MotionEvent {
                    location,
                    serial,
                    time,
                },
            );
            pointer.frame(&mut self.state);
        }
    }

    pub fn pointer_button(&mut self, button: u32, btn_state: ButtonState) {
        let serial = next_serial();
        let time = self.now_ms();
        if let Some(pointer) = self.state.seat.get_pointer() {
            pointer.button(
                &mut self.state,
                &ButtonEvent {
                    button,
                    state: btn_state,
                    serial,
                    time,
                },
            );
            pointer.frame(&mut self.state);
        }
    }

    pub fn pointer_axis(&mut self, horizontal: f64, vertical: f64, discrete: Option<(i32, i32)>) {
        let time = self.now_ms();
        let Some(pointer) = self.state.seat.get_pointer() else {
            return;
        };

        let source = if discrete.is_some() {
            AxisSource::Wheel
        } else {
            AxisSource::Finger
        };
        let mut frame = AxisFrame::new(time).source(source);
        if horizontal != 0.0 {
            frame = frame.value(Axis::Horizontal, horizontal);
        }
        if vertical != 0.0 {
            frame = frame.value(Axis::Vertical, vertical);
        }
        if let Some((h, v)) = discrete {
            if h != 0 {
                frame = frame.v120(Axis::Horizontal, h);
            }
            if v != 0 {
                frame = frame.v120(Axis::Vertical, v);
            }
        }

        pointer.axis(&mut self.state, frame);
        pointer.frame(&mut self.state);
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

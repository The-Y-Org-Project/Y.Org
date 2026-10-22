use smithay::{
    backend::{
        input::{ButtonState, KeyState},
        renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state},
    },
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, xkb},
        pointer::{ButtonEvent, MotionEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
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
        },
        shm::{BufferData, ShmHandler, ShmState, with_buffer_contents},
    },
};
use std::error::Error;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Instant;

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

struct WaylandState {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    _output: Output,
    pub seat: Seat<Self>,
    updates: Vec<(ObjectId, SurfacePixels)>,
    popups: Vec<PopupSurface>,
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

fn window_geometry_origin(surface: &WlSurface) -> (i32, i32) {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
            .map(|g| (g.loc.x, g.loc.y))
            .unwrap_or((0, 0))
    })
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

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
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
            shm_state: ShmState::new::<WaylandState>(&dh, vec![]),
            seat_state,
            data_device_state,
            _output: output,
            seat,
            updates: Vec::new(),
            popups: Vec::new(),
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

    pub fn popup_infos(&mut self) -> Vec<PopupInfo> {
        self.state.popups.retain(|p| p.wl_surface().is_alive());
        self.state
            .popups
            .iter()
            .filter_map(|popup| {
                let parent = popup.get_parent_surface()?;
                let geometry = popup.with_pending_state(|state| state.geometry);
                let (parent_x, parent_y) = window_geometry_origin(&parent);
                let (own_x, own_y) = window_geometry_origin(popup.wl_surface());
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
        self.state
            .popups
            .iter()
            .any(|p| p.wl_surface().is_alive())
    }

    pub fn dismiss_popups(&mut self) {
        for popup in &self.state.popups {
            popup.send_popup_done();
        }
    }

    pub fn take_updates(&mut self) -> Vec<(ObjectId, SurfacePixels)> {
        std::mem::take(&mut self.state.updates)
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
        let keycode = xkb::Keycode::new(scancode + 8);

        if let Some(keyboard) = self.state.seat.get_keyboard() {
            let _ = keyboard.input(
                &mut self.state,
                keycode,
                key_state,
                serial,
                self.started.elapsed().as_millis() as u32,
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
                    time: self.started.elapsed().as_millis() as u32,
                },
            );
            pointer.frame(&mut self.state);
        }
    }

    pub fn pointer_button(&mut self, button: u32, btn_state: ButtonState) {
        let serial = next_serial();
        if let Some(pointer) = self.state.seat.get_pointer() {
            pointer.button(
                &mut self.state,
                &ButtonEvent {
                    button,
                    state: btn_state,
                    serial,
                    time: self.started.elapsed().as_millis() as u32,
                },
            );
            pointer.frame(&mut self.state);
        }
    }

    pub fn resize_toplevel(&mut self, id: ObjectId, width: i32, height: i32) {
        if let Some(surface) = self
            .state
            .xdg_shell_state
            .toplevel_surfaces()
            .iter()
            .find(|s| s.wl_surface().id() == id)
        {
            surface.with_pending_state(|state| {
                state.size = Some((width, height).into());
            });
            surface.send_configure();
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

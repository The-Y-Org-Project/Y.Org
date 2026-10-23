use std::collections::HashMap;
use std::os::unix::io::AsRawFd;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver};
use vte::{Parser, Perform};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

const FONT_SIZE: f32 = 16.0;
const FONT_PATH: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";
const SCROLLBACK_LINES: usize = 2000;
const POLL_TIMEOUT_MS: i32 = 4;

#[derive(Clone, Copy)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
}

impl Color {
    const DEFAULT_FG: Self = Self { r: 220, g: 220, b: 220 };
    const DEFAULT_BG: Self = Self { r: 30, g: 30, b: 30 };
    const CURSOR: Self = Self { r: 200, g: 200, b: 200 };
}

#[derive(Clone, Copy)]
struct Cell {
    c: char,
    fg: Color,
    bg: Color,
}

impl Default for Cell {
    fn default() -> Self {
        Self { c: ' ', fg: Color::DEFAULT_FG, bg: Color::DEFAULT_BG }
    }
}

struct Grid {
    cells: Vec<Vec<Cell>>,
    scrollback: Vec<Vec<Cell>>,
    cursor_x: usize,
    cursor_y: usize,
    width: usize,
    height: usize,
    fg: Color,
    bg: Color,
    bold: bool,
    scroll_offset: usize,
}

impl Grid {
    fn new(width: usize, height: usize) -> Self {
        Self {
            cells: vec![vec![Cell::default(); width]; height],
            scrollback: Vec::new(),
            cursor_x: 0,
            cursor_y: 0,
            width,
            height,
            fg: Color::DEFAULT_FG,
            bg: Color::DEFAULT_BG,
            bold: false,
            scroll_offset: 0,
        }
    }

    fn clear_screen(&mut self) {
        for row in &mut self.cells {
            for cell in row {
                *cell = Cell::default();
            }
        }
        self.fg = Color::DEFAULT_FG;
        self.bg = Color::DEFAULT_BG;
        self.bold = false;
    }

    fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.cells = vec![vec![Cell::default(); width]; height];
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn scroll_up_one(&mut self) {
        if !self.cells.is_empty() {
            let row = self.cells.remove(0);
            self.scrollback.push(row);
            if self.scrollback.len() > SCROLLBACK_LINES {
                self.scrollback.remove(0);
            }
            self.cells.push(vec![Cell::default(); self.width]);
        }
    }
}

struct TerminalPerformer {
    grid: Grid,
    changed: bool,
}

impl TerminalPerformer {
    fn newline(&mut self) {
        self.grid.cursor_y += 1;
        self.grid.cursor_x = 0;
        if self.grid.cursor_y >= self.grid.height {
            self.grid.scroll_up_one();
            self.grid.cursor_y = self.grid.height.saturating_sub(1);
            self.changed = true;
        }
    }
}

impl Perform for TerminalPerformer {
    fn print(&mut self, c: char) {
        if self.grid.cursor_y < self.grid.height && self.grid.cursor_x < self.grid.width {
            let fg = if self.grid.bold { brighten(self.grid.fg) } else { self.grid.fg };
            self.grid.cells[self.grid.cursor_y][self.grid.cursor_x] = Cell {
                c,
                fg,
                bg: self.grid.bg,
            };
            self.grid.cursor_x += 1;
            self.changed = true;
        }
        if self.grid.cursor_x >= self.grid.width {
            self.newline();
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.grid.cursor_x = 0,
            b'\t' => {
                let next = ((self.grid.cursor_x / 8) + 1) * 8;
                self.grid.cursor_x = next.min(self.grid.width.saturating_sub(1));
            }
            0x08 => {
                if self.grid.cursor_x > 0 {
                    self.grid.cursor_x -= 1;
                }
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, _intermediates: &[u8], _ignore: bool, action: char) {
        let p: Vec<u16> = params.iter().flat_map(|chunk| chunk.iter().copied()).collect();
        let first = p.first().copied().unwrap_or(0);

        match action {
            'J' => {
                match first {
                    0 => {
                        for x in self.grid.cursor_x..self.grid.width {
                            self.grid.cells[self.grid.cursor_y][x] = Cell::default();
                        }
                    }
                    1 => {
                        for x in 0..=self.grid.cursor_x.min(self.grid.width.saturating_sub(1)) {
                            self.grid.cells[self.grid.cursor_y][x] = Cell::default();
                        }
                    }
                    2 => self.grid.clear_screen(),
                    3 => self.grid.scrollback.clear(),
                    _ => {}
                }
                self.changed = true;
            }
            'K' => {
                match first {
                    0 => {
                        for x in self.grid.cursor_x..self.grid.width {
                            self.grid.cells[self.grid.cursor_y][x] = Cell::default();
                        }
                    }
                    1 => {
                        for x in 0..=self.grid.cursor_x.min(self.grid.width.saturating_sub(1)) {
                            self.grid.cells[self.grid.cursor_y][x] = Cell::default();
                        }
                    }
                    2 => {
                        for x in 0..self.grid.width {
                            self.grid.cells[self.grid.cursor_y][x] = Cell::default();
                        }
                    }
                    _ => {}
                }
                self.changed = true;
            }
            'H' | 'f' => {
                let row = p.get(0).copied().unwrap_or(1).saturating_sub(1) as usize;
                let col = p.get(1).copied().unwrap_or(1).saturating_sub(1) as usize;
                self.grid.cursor_y = row.min(self.grid.height.saturating_sub(1));
                self.grid.cursor_x = col.min(self.grid.width.saturating_sub(1));
                self.changed = true;
            }
            'A' => {
                let n = first.max(1) as usize;
                self.grid.cursor_y = self.grid.cursor_y.saturating_sub(n);
                self.changed = true;
            }
            'B' => {
                let n = first.max(1) as usize;
                self.grid.cursor_y = (self.grid.cursor_y + n).min(self.grid.height.saturating_sub(1));
                self.changed = true;
            }
            'C' => {
                let n = first.max(1) as usize;
                self.grid.cursor_x = (self.grid.cursor_x + n).min(self.grid.width.saturating_sub(1));
                self.changed = true;
            }
            'D' => {
                let n = first.max(1) as usize;
                self.grid.cursor_x = self.grid.cursor_x.saturating_sub(n);
                self.changed = true;
            }
            'm' => {
                if p.is_empty() {
                    self.grid.fg = Color::DEFAULT_FG;
                    self.grid.bg = Color::DEFAULT_BG;
                    self.grid.bold = false;
                } else {
                    let mut i = 0;
                    while i < p.len() {
                        match p[i] {
                            0 => {
                                self.grid.fg = Color::DEFAULT_FG;
                                self.grid.bg = Color::DEFAULT_BG;
                                self.grid.bold = false;
                            }
                            1 => self.grid.bold = true,
                            22 => self.grid.bold = false,
                            30..=37 => self.grid.fg = ansi_color((p[i] - 30) as u8),
                            39 => self.grid.fg = Color::DEFAULT_FG,
                            40..=47 => self.grid.bg = ansi_color((p[i] - 40) as u8),
                            49 => self.grid.bg = Color::DEFAULT_BG,
                            90..=97 => self.grid.fg = ansi_bright_color((p[i] - 90) as u8),
                            100..=107 => self.grid.bg = ansi_bright_color((p[i] - 100) as u8),
                            38 if i + 2 < p.len() && p[i + 1] == 5 => {
                                self.grid.fg = color_256(p[i + 2] as u8);
                                i += 2;
                            }
                            48 if i + 2 < p.len() && p[i + 1] == 5 => {
                                self.grid.bg = color_256(p[i + 2] as u8);
                                i += 2;
                            }
                            38 if i + 4 < p.len() && p[i + 1] == 2 => {
                                self.grid.fg = Color {
                                    r: p[i + 2] as u8,
                                    g: p[i + 3] as u8,
                                    b: p[i + 4] as u8,
                                };
                                i += 4;
                            }
                            48 if i + 4 < p.len() && p[i + 1] == 2 => {
                                self.grid.bg = Color {
                                    r: p[i + 2] as u8,
                                    g: p[i + 3] as u8,
                                    b: p[i + 4] as u8,
                                };
                                i += 4;
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                self.changed = true;
            }
            _ => {}
        }
    }
}

fn ansi_color(i: u8) -> Color {
    const COLORS: [(u8, u8, u8); 8] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
    ];
    let (r, g, b) = COLORS[(i as usize).min(7)];
    Color { r, g, b }
}

fn ansi_bright_color(i: u8) -> Color {
    const COLORS: [(u8, u8, u8); 8] = [
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    let (r, g, b) = COLORS[(i as usize).min(7)];
    Color { r, g, b }
}

fn brighten(c: Color) -> Color {
    Color {
        r: (c.r as u32 + 40).min(255) as u8,
        g: (c.g as u32 + 40).min(255) as u8,
        b: (c.b as u32 + 40).min(255) as u8,
    }
}

fn color_256(i: u8) -> Color {
    match i {
        0..=15 => {
            if i < 8 { ansi_color(i) } else { ansi_bright_color(i - 8) }
        }
        16..=231 => {
            let idx = i - 16;
            Color {
                r: (idx / 36) * 51,
                g: ((idx / 6) % 6) * 51,
                b: (idx % 6) * 51,
            }
        }
        _ => {
            let v = (8 + (i - 232) * 10).min(255);
            Color { r: v, g: v, b: v }
        }
    }
}

struct CachedGlyph {
    bounds_min_x: i32,
    bounds_min_y: i32,
    width: u32,
    height: u32,
    bitmap: Vec<u8>,
}

struct GlyphCache {
    glyphs: HashMap<char, Option<CachedGlyph>>,
}

fn rasterize_glyph(scaled: &ab_glyph::PxScaleFont<&FontVec>, c: char) -> Option<CachedGlyph> {
    let glyph = scaled.scaled_glyph(c);
    let outline = scaled.outline_glyph(glyph)?;
    let bounds = outline.px_bounds();
    let w = bounds.width() as u32;
    let h = bounds.height() as u32;
    if w == 0 || h == 0 {
        return None;
    }
    let mut bitmap = vec![0u8; (w * h) as usize];
    outline.draw(|x, y, cov| {
        let idx = (y * w + x) as usize;
        if idx < bitmap.len() {
            bitmap[idx] = (cov * 255.0).min(255.0) as u8;
        }
    });
    Some(CachedGlyph {
        bounds_min_x: bounds.min.x as i32,
        bounds_min_y: bounds.min.y as i32,
        width: w,
        height: h,
        bitmap,
    })
}

impl GlyphCache {
    fn new(font: &FontVec, scale: PxScale) -> Self {
        let mut glyphs = HashMap::new();
        let scaled = font.as_scaled(scale);
        for code in 32u8..=126 {
            let c = code as char;
            glyphs.insert(c, rasterize_glyph(&scaled, c));
        }
        for code in 0xC0u32..=0xFF {
            if let Some(c) = char::from_u32(code) {
                glyphs.insert(c, rasterize_glyph(&scaled, c));
            }
        }
        Self { glyphs }
    }

    fn get(&self, c: char) -> Option<&CachedGlyph> {
        self.glyphs.get(&c).and_then(|x| x.as_ref())
    }
}

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");
    let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh).unwrap();
    let xdg_shell_state = XdgShell::bind(&globals, &qh).unwrap();
    let shm_state = Shm::bind(&globals, &qh).unwrap();

    let font_data = std::fs::read(FONT_PATH).expect("Font not found");
    let font = FontVec::try_from_vec(font_data).unwrap();

    let scale = PxScale::from(FONT_SIZE);
    let scaled = font.as_scaled(scale);
    let char_w = scaled.h_advance(scaled.glyph_id('M')).ceil() as i32;
    let line_h = (scaled.height() + scaled.line_gap()).ceil() as i32;
    let ascent = scaled.ascent().ceil() as i32;

    let glyph_cache = GlyphCache::new(&font, scale);

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor_state,
        xdg_shell_state,
        shm_state,
        window: None,
        pool: None,
        keyboard: None,
        pointer: None,
        exit: false,
        grid: Grid::new(80, 24),
        glyph_cache,
        parser: Parser::new(),
        pty_tx: None,
        pty_rx: None,
        current_size: (0, 0),
        char_w,
        line_h,
        ascent,
    };

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new("/bin/bash");
    cmd.env("TERM", "xterm-256color");
    let _child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = pair.master.take_writer().unwrap();

    let (tx, rx) = channel();
    app.pty_tx = Some(writer);
    app.pty_rx = Some(rx);

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });

    event_queue.roundtrip(&mut app).unwrap();

    let surface = app.compositor_state.create_surface(&qh);
    let window = app
        .xdg_shell_state
        .create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("y-terminal");
    window.commit();

    let pool = SlotPool::new(800 * 600 * 4, &app.shm_state).unwrap();
    app.window = Some(window);
    app.pool = Some(pool);

    while !app.exit {
        let mut needs_redraw = false;

        if let Some(rx) = &app.pty_rx {
            let mut all_bytes = Vec::new();
            while let Ok(bytes) = rx.try_recv() {
                all_bytes.extend_from_slice(&bytes);
            }
            if !all_bytes.is_empty() {
                let mut performer = TerminalPerformer {
                    grid: std::mem::replace(&mut app.grid, Grid::new(0, 0)),
                    changed: false,
                };
                for &byte in &all_bytes {
                    app.parser.advance(&mut performer, byte);
                }
                app.grid = performer.grid;
                if performer.changed {
                    needs_redraw = true;
                }
            }
        }

        if needs_redraw {
            let (w, h) = app.current_size;
            if w > 0 && h > 0 {
                app.draw(w, h);
                if let Some(window) = &app.window {
                    window.wl_surface().commit();
                }
            }
        }

        event_queue.dispatch_pending(&mut app).unwrap();
        event_queue.flush().unwrap();

        if let Some(guard) = event_queue.prepare_read() {
            let mut pollfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ret = unsafe { libc::poll(&mut pollfd, 1, POLL_TIMEOUT_MS) };
            if ret > 0 && (pollfd.revents & libc::POLLIN) != 0 {
                let _ = guard.read();
            }
        }
    }
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShell,
    shm_state: Shm,
    window: Option<Window>,
    pool: Option<SlotPool>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    exit: bool,
    grid: Grid,
    glyph_cache: GlyphCache,
    parser: Parser,
    pty_tx: Option<Box<dyn Write + Send>>,
    pty_rx: Option<Receiver<Vec<u8>>>,
    current_size: (u32, u32),
    char_w: i32,
    line_h: i32,
    ascent: i32,
}

impl App {
    fn draw(&mut self, width: u32, height: u32) {
        let char_w = self.char_w;
        let line_h = self.line_h;
        let ascent = self.ascent;

        let Self { pool, grid, glyph_cache, window, .. } = self;

        let pool = pool.as_mut().unwrap();
        let (buffer, canvas) = pool
            .create_buffer(
                width as i32,
                height as i32,
                width as i32 * 4,
                wl_shm::Format::Argb8888,
            )
            .unwrap();

        let bg_px = [Color::DEFAULT_BG.b, Color::DEFAULT_BG.g, Color::DEFAULT_BG.r, 255];
        for chunk in canvas.chunks_exact_mut(4) {
            chunk.copy_from_slice(&bg_px);
        }

        for row in 0..grid.height {
            let content_row = grid.scrollback.len() + row;
            let content_row = content_row as i64 - grid.scroll_offset as i64;
            if content_row < 0 {
                continue;
            }
            let content_row = content_row as usize;

            let cells: &[Cell] = if content_row < grid.scrollback.len() {
                &grid.scrollback[content_row]
            } else {
                let grid_row = content_row - grid.scrollback.len();
                if grid_row >= grid.height {
                    continue;
                }
                &grid.cells[grid_row]
            };

            for (col, cell) in cells.iter().enumerate() {
                let has_bg = !(cell.bg.r == Color::DEFAULT_BG.r
                    && cell.bg.g == Color::DEFAULT_BG.g
                    && cell.bg.b == Color::DEFAULT_BG.b);

                let cell_x = (col as i32) * char_w;
                let cell_y = (row as i32) * line_h;

                if has_bg {
                    let bg_cell = [cell.bg.b, cell.bg.g, cell.bg.r, 255];
                    for py in cell_y..(cell_y + line_h).min(height as i32) {
                        for px in cell_x..(cell_x + char_w).min(width as i32) {
                            if px >= 0 && py >= 0 {
                                let idx = ((py * width as i32 + px) * 4) as usize;
                                canvas[idx..idx + 4].copy_from_slice(&bg_cell);
                            }
                        }
                    }
                }

                if cell.c == ' ' {
                    continue;
                }

                if let Some(cached) = glyph_cache.get(cell.c) {
                    let fg = cell.fg;
                    let base_x = cell_x + cached.bounds_min_x;
                    let base_y = cell_y + ascent + cached.bounds_min_y;
                    for gy in 0..cached.height {
                        for gx in 0..cached.width {
                            let px = base_x + gx as i32;
                            let py = base_y + gy as i32;
                            if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                                let cov = cached.bitmap[(gy * cached.width + gx) as usize] as f32 / 255.0;
                                if cov > 0.0 {
                                    let idx = ((py * width as i32 + px) * 4) as usize;
                                    canvas[idx] = (canvas[idx] as f32 * (1.0 - cov) + fg.b as f32 * cov) as u8;
                                    canvas[idx + 1] = (canvas[idx + 1] as f32 * (1.0 - cov) + fg.g as f32 * cov) as u8;
                                    canvas[idx + 2] = (canvas[idx + 2] as f32 * (1.0 - cov) + fg.r as f32 * cov) as u8;
                                }
                            }
                        }
                    }
                }
            }
        }

        if grid.scroll_offset == 0 {
            let cx = (grid.cursor_x as i32) * char_w;
            let cy = (grid.cursor_y as i32) * line_h;
            let cur_px = [Color::CURSOR.b, Color::CURSOR.g, Color::CURSOR.r, 255];
            for y in cy..(cy + line_h - 2).min(height as i32) {
                for x in cx..(cx + char_w).min(width as i32) {
                    if x >= 0 && y >= 0 {
                        let idx = ((y * width as i32 + x) * 4) as usize;
                        canvas[idx..idx + 4].copy_from_slice(&cur_px);
                    }
                }
            }
        }

        let surface = window.as_ref().unwrap().wl_surface();
        buffer.attach_to(surface).unwrap();
        surface.damage_buffer(0, 0, width as i32, height as i32);
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        let width = configure.new_size.0.map(|v| v.get()).unwrap_or(800);
        let height = configure.new_size.1.map(|v| v.get()).unwrap_or(600);
        let cols = (width as i32 / self.char_w).max(1) as usize;
        let rows = (height as i32 / self.line_h).max(1) as usize;
        if cols != self.grid.width || rows != self.grid.height {
            self.grid.resize(cols, rows);
        }
        self.current_size = (width, height);
        self.draw(width, height);
        if let Some(window) = &self.window {
            window.wl_surface().commit();
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            if let Ok(kb) = self.seat_state.get_keyboard(qh, &seat, None) {
                self.keyboard = Some(kb);
            }
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            if let Ok(ptr) = self.seat_state.get_pointer(qh, &seat) {
                self.pointer = Some(ptr);
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_some() {
            self.keyboard.take().unwrap().release();
        }
        if capability == Capability::Pointer && self.pointer.is_some() {
            self.pointer.take().unwrap().release();
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {}

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if self.grid.scroll_offset > 0 {
            self.grid.scroll_offset = 0;
            let (w, h) = self.current_size;
            if w > 0 && h > 0 {
                self.draw(w, h);
                if let Some(window) = &self.window {
                    window.wl_surface().commit();
                }
            }
        }
        if let Some(tx) = &mut self.pty_tx {
            if let Some(text) = &event.utf8 {
                tx.write_all(text.as_bytes()).unwrap();
            } else {
                match event.keysym {
                    Keysym::Return => tx.write_all(b"\r").unwrap(),
                    Keysym::BackSpace => tx.write_all(b"\x7f").unwrap(),
                    Keysym::Tab => tx.write_all(b"\t").unwrap(),
                    Keysym::Escape => tx.write_all(b"\x1b").unwrap(),
                    Keysym::Up => tx.write_all(b"\x1b[A").unwrap(),
                    Keysym::Down => tx.write_all(b"\x1b[B").unwrap(),
                    Keysym::Right => tx.write_all(b"\x1b[C").unwrap(),
                    Keysym::Left => tx.write_all(b"\x1b[D").unwrap(),
                    Keysym::Home => tx.write_all(b"\x1b[H").unwrap(),
                    Keysym::End => tx.write_all(b"\x1b[F").unwrap(),
                    _ => {}
                }
            }
            tx.flush().unwrap();
        }
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: Modifiers, _: u32) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if let PointerEventKind::Axis { vertical, .. } = &event.kind {
                let delta: i32 = if vertical.discrete != 0 {
                    vertical.discrete.signum() * 3
                } else {
                    let v = vertical.absolute;
                    if v > 0.0 { 3 } else if v < 0.0 { -3 } else { 0 }
                };

                if delta > 0 {
                    self.grid.scroll_offset =
                        (self.grid.scroll_offset + delta as usize).min(self.grid.scrollback.len());
                } else if delta < 0 {
                    self.grid.scroll_offset =
                        self.grid.scroll_offset.saturating_sub((-delta) as usize);
                } else {
                    continue;
                }

                let (w, h) = self.current_size;
                if w > 0 && h > 0 {
                    self.draw(w, h);
                    if let Some(window) = &self.window {
                        window.wl_surface().commit();
                    }
                }
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_shm!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_registry!(App);

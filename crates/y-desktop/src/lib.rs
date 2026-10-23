use y_gpu::{DrawCall, SOLID_TEXTURE, TextureId, Vertex};

pub const SOLID_TEXEL: [f32; 2] = [0.5 / 64.0, 0.5 / 64.0];
const DOCK_CELL: (f32, f32) = (160.0, 100.0);
const DOCK_GAP: f32 = 10.0;
const DOCK_BORDER: f32 = 4.0;
const DOCK_BORDER_COLOR: [f32; 3] = [0.30, 0.35, 0.55];
const DOCK_BAR_HEIGHT: f32 = 4.0;
const DOCK_BAR_COLOR: [f32; 3] = [0.45, 0.50, 0.70];
const DOCK_BAR_TRACK_COLOR: [f32; 3] = [0.16, 0.18, 0.28];
const DOCK_BAR_MIN_THUMB: f32 = 24.0;

const LAUNCHER_WIDTH: f32 = 60.0;
const LAUNCHER_GAP: f32 = 10.0;
const LAUNCHER_COLOR: [f32; 3] = [0.35, 0.55, 0.85];
const LAUNCHER_PLUS_COLOR: [f32; 3] = [0.95, 0.95, 0.95];

pub const TITLE_BAR_HEIGHT: f32 = 30.0;
const BUTTON_SIZE: f32 = 16.0;
const BUTTON_MARGIN: f32 = 7.0;
const TITLE_BAR_COLOR: [f32; 3] = [0.20, 0.22, 0.28];
const CLOSE_COLOR: [f32; 3] = [0.85, 0.30, 0.30];
const MINIMIZE_COLOR: [f32; 3] = [0.85, 0.75, 0.30];
const MAXIMIZE_COLOR: [f32; 3] = [0.30, 0.75, 0.40];
const BORDER_COLOR: [f32; 3] = [0.35, 0.35, 0.45];

const UNFOCUSED_BRIGHTNESS: f32 = 0.7;
const RESIZE_EDGE: f32 = 10.0;
const RESIZE_OUTER: f32 = 4.0;
pub const EDGE_LEFT: u8 = 1;
pub const EDGE_RIGHT: u8 = 2;
pub const EDGE_TOP: u8 = 4;
pub const EDGE_BOTTOM: u8 = 8;

pub struct WindowInfo {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: [f32; 3],
    pub texture: Option<TextureId>,
    pub minimized: bool,
    pub maximized: bool,
    pub has_title_bar: bool,
    pub focused: bool,
}

pub enum TitleBarHit {
    Close(u32),
    Minimize(u32),
    Maximize(u32),
    Drag(u32),
}

impl WindowInfo {
    fn look(&self) -> (TextureId, [f32; 3], [f32; 2], [f32; 2]) {
        match self.texture {
            Some(t) => (t, [1.0, 1.0, 1.0], [0.0, 0.0], [1.0, 1.0]),
            None => (SOLID_TEXTURE, self.color, SOLID_TEXEL, SOLID_TEXEL),
        }
    }
}

pub struct Desktop {
    dock_scroll: f32,
}

impl Default for Desktop {
    fn default() -> Self {
        Self::new()
    }
}

impl Desktop {
    pub fn new() -> Self {
        Self { dock_scroll: 0.0 }
    }

    fn launcher_end() -> f32 {
        DOCK_GAP + LAUNCHER_WIDTH + LAUNCHER_GAP
    }

    fn launcher_rect(screen: (f32, f32)) -> [f32; 4] {
        let x = DOCK_GAP;
        let y = screen.1 - DOCK_CELL.1 - DOCK_GAP;
        [x, y, LAUNCHER_WIDTH, DOCK_CELL.1]
    }

    pub fn launcher_hit(&self, px: f32, py: f32, screen: (f32, f32)) -> bool {
        let [x, y, w, h] = Self::launcher_rect(screen);
        px >= x && px < x + w && py >= y && py < y + h
    }

    fn dock_count(windows: &[WindowInfo]) -> usize {
        windows.iter().filter(|w| w.minimized).count()
    }

    fn dock_content_width(windows: &[WindowInfo]) -> f32 {
        let count = Self::dock_count(windows);
        if count == 0 {
            0.0
        } else {
            count as f32 * (DOCK_CELL.0 + DOCK_GAP)
        }
    }

    fn dock_max_scroll(windows: &[WindowInfo], screen: (f32, f32)) -> f32 {
        let visible = (screen.0 - Self::launcher_end()).max(1.0);
        (Self::dock_content_width(windows) - visible).max(0.0)
    }

    fn dock_offset(&self, windows: &[WindowInfo], screen: (f32, f32)) -> f32 {
        self.dock_scroll
            .clamp(0.0, Self::dock_max_scroll(windows, screen))
    }

    pub fn scrollable(&self, windows: &[WindowInfo], screen: (f32, f32)) -> bool {
        Self::dock_max_scroll(windows, screen) > 0.0
    }

    pub fn scroll(&mut self, delta: f32, windows: &[WindowInfo], screen: (f32, f32)) -> bool {
        let max = Self::dock_max_scroll(windows, screen);
        let next = (self.dock_scroll + delta).clamp(0.0, max);
        let changed = (next - self.dock_scroll).abs() > f32::EPSILON;
        self.dock_scroll = next;
        changed
    }

    pub fn over(&self, px: f32, py: f32, windows: &[WindowInfo], screen: (f32, f32)) -> bool {
        if Self::dock_count(windows) == 0 {
            return false;
        }
        let top = screen.1 - DOCK_CELL.1 - DOCK_GAP * 2.0 - DOCK_BORDER;
        let left = Self::launcher_end();
        px >= left && px < screen.0 && py >= top && py < screen.1
    }

    pub fn hit(&self, px: f32, py: f32, windows: &[WindowInfo], screen: (f32, f32)) -> Option<u32> {
        let y = screen.1 - DOCK_CELL.1 - DOCK_GAP;
        let offset = self.dock_offset(windows, screen);
        let start_x = Self::launcher_end();
        for (i, w) in windows.iter().filter(|w| w.minimized).enumerate() {
            let x = start_x + i as f32 * (DOCK_CELL.0 + DOCK_GAP) - offset;
            if x + DOCK_CELL.0 < start_x || x > screen.0 {
                continue;
            }
            if px >= x && px < x + DOCK_CELL.0 && py >= y && py < y + DOCK_CELL.1 {
                return Some(w.id);
            }
        }
        None
    }

    pub fn edge_hit(
        &self,
        px: f32,
        py: f32,
        windows: &[WindowInfo],
        _screen: (f32, f32),
    ) -> Option<(u32, u8)> {
        for w in windows
            .iter()
            .rev()
            .filter(|w| !w.minimized && !w.maximized && w.has_title_bar)
        {
            let body_top = w.y - TITLE_BAR_HEIGHT;
            let body_bottom = w.y + w.height;
            let body_left = w.x;
            let body_right = w.x + w.width;

            let outer_left = body_left - RESIZE_OUTER;
            let outer_right = body_right + RESIZE_OUTER;
            let outer_top = body_top - RESIZE_OUTER;
            let outer_bottom = body_bottom + RESIZE_OUTER;

            if px < outer_left || px >= outer_right || py < outer_top || py >= outer_bottom {
                continue;
            }

            let mut edges = 0u8;
            if px - body_left < RESIZE_EDGE {
                edges |= EDGE_LEFT;
            }
            if body_right - px < RESIZE_EDGE {
                edges |= EDGE_RIGHT;
            }
            if py - body_top < RESIZE_EDGE {
                edges |= EDGE_TOP;
            }
            if body_bottom - py < RESIZE_EDGE {
                edges |= EDGE_BOTTOM;
            }

            if edges != 0 {
                return Some((w.id, edges));
            }

            return None;
        }
        None
    }

    pub fn title_bar_hit(
        &self,
        px: f32,
        py: f32,
        windows: &[WindowInfo],
        _screen: (f32, f32),
    ) -> Option<TitleBarHit> {
        for w in windows.iter().rev().filter(|w| !w.minimized && w.has_title_bar) {
            let top = w.y - TITLE_BAR_HEIGHT;
            if px >= w.x && px < w.x + w.width && py >= top && py < w.y {
                let btn_y = top + (TITLE_BAR_HEIGHT - BUTTON_SIZE) / 2.0;
                let close_x = w.x + w.width - BUTTON_SIZE - BUTTON_MARGIN;
                if px >= close_x
                    && px < close_x + BUTTON_SIZE
                    && py >= btn_y
                    && py < btn_y + BUTTON_SIZE
                {
                    return Some(TitleBarHit::Close(w.id));
                }
                let max_x = close_x - BUTTON_SIZE - BUTTON_MARGIN;
                if px >= max_x
                    && px < max_x + BUTTON_SIZE
                    && py >= btn_y
                    && py < btn_y + BUTTON_SIZE
                {
                    return Some(TitleBarHit::Maximize(w.id));
                }
                let min_x = max_x - BUTTON_SIZE - BUTTON_MARGIN;
                if px >= min_x
                    && px < min_x + BUTTON_SIZE
                    && py >= btn_y
                    && py < btn_y + BUTTON_SIZE
                {
                    return Some(TitleBarHit::Minimize(w.id));
                }
                return Some(TitleBarHit::Drag(w.id));
            }
        }
        None
    }

    pub fn scene(&self, windows: &[WindowInfo], screen: (f32, f32)) -> (Vec<Vertex>, Vec<DrawCall>) {
        let mut vertices = Vec::new();
        let mut draws = Vec::new();

        let y = screen.1 - DOCK_CELL.1 - DOCK_GAP;
        let offset = self.dock_offset(windows, screen);
        let start_x = Self::launcher_end();

        for (i, w) in windows.iter().filter(|w| w.minimized).enumerate() {
            let x = start_x + i as f32 * (DOCK_CELL.0 + DOCK_GAP) - offset;
            if x + DOCK_CELL.0 < 0.0 || x > screen.0 {
                continue;
            }
            push_quad(
                &mut vertices,
                &mut draws,
                [
                    x - DOCK_BORDER,
                    y - DOCK_BORDER,
                    DOCK_CELL.0 + DOCK_BORDER * 2.0,
                    DOCK_CELL.1 + DOCK_BORDER * 2.0,
                ],
                DOCK_BORDER_COLOR,
                SOLID_TEXEL,
                SOLID_TEXEL,
                SOLID_TEXTURE,
            );
            let (texture, color, uv0, uv1) = w.look();
            let scale = if w.width > 0.0 && w.height > 0.0 {
                (DOCK_CELL.0 / w.width).min(DOCK_CELL.1 / w.height)
            } else {
                1.0
            };
            let tw = w.width * scale;
            let th = w.height * scale;
            push_quad(
                &mut vertices,
                &mut draws,
                [
                    x + (DOCK_CELL.0 - tw) / 2.0,
                    y + (DOCK_CELL.1 - th) / 2.0,
                    tw,
                    th,
                ],
                color,
                uv0,
                uv1,
                texture,
            );
        }

        let [lx, ly, lw, lh] = Self::launcher_rect(screen);
        push_quad(
            &mut vertices,
            &mut draws,
            [lx, ly, lw, lh],
            LAUNCHER_COLOR,
            SOLID_TEXEL,
            SOLID_TEXEL,
            SOLID_TEXTURE,
        );

        let plus_size = LAUNCHER_WIDTH.min(DOCK_CELL.1) * 0.35;
        let plus_x = lx + (LAUNCHER_WIDTH - plus_size) / 2.0;
        let plus_y = ly + (DOCK_CELL.1 - plus_size) / 2.0;
        let bar = (plus_size * 0.20).max(2.0);

        push_quad(
            &mut vertices,
            &mut draws,
            [plus_x, plus_y + (plus_size - bar) / 2.0, plus_size, bar],
            LAUNCHER_PLUS_COLOR,
            SOLID_TEXEL,
            SOLID_TEXEL,
            SOLID_TEXTURE,
        );
        push_quad(
            &mut vertices,
            &mut draws,
            [plus_x + (plus_size - bar) / 2.0, plus_y, bar, plus_size],
            LAUNCHER_PLUS_COLOR,
            SOLID_TEXEL,
            SOLID_TEXEL,
            SOLID_TEXTURE,
        );

        let max_scroll = Self::dock_max_scroll(windows, screen);
        if max_scroll > 0.0 {
            let track_x = start_x;
            let track_width = (screen.0 - start_x - DOCK_GAP).max(1.0);
            let bar_y = screen.1 - DOCK_GAP + 3.0;
            push_quad(
                &mut vertices,
                &mut draws,
                [track_x, bar_y, track_width, DOCK_BAR_HEIGHT],
                DOCK_BAR_TRACK_COLOR,
                SOLID_TEXEL,
                SOLID_TEXEL,
                SOLID_TEXTURE,
            );

            let visible = (screen.0 - start_x).max(1.0);
            let thumb_width = (track_width * visible / Self::dock_content_width(windows))
                .max(DOCK_BAR_MIN_THUMB);
            let thumb_x = track_x
                + (track_width - thumb_width)
                    * (self.dock_offset(windows, screen) / max_scroll);
            push_quad(
                &mut vertices,
                &mut draws,
                [thumb_x, bar_y, thumb_width, DOCK_BAR_HEIGHT],
                DOCK_BAR_COLOR,
                SOLID_TEXEL,
                SOLID_TEXEL,
                SOLID_TEXTURE,
            );
        }

        for w in windows.iter().filter(|w| !w.minimized) {
            let f = if w.focused { 1.0 } else { UNFOCUSED_BRIGHTNESS };

            if w.has_title_bar {
                let top = w.y - TITLE_BAR_HEIGHT;
                let border = [
                    BORDER_COLOR[0] * f,
                    BORDER_COLOR[1] * f,
                    BORDER_COLOR[2] * f,
                ];
                let title = [
                    TITLE_BAR_COLOR[0] * f,
                    TITLE_BAR_COLOR[1] * f,
                    TITLE_BAR_COLOR[2] * f,
                ];

                push_quad(
                    &mut vertices,
                    &mut draws,
                    [
                        w.x - 1.0,
                        top - 1.0,
                        w.width + 2.0,
                        w.height + TITLE_BAR_HEIGHT + 2.0,
                    ],
                    border,
                    SOLID_TEXEL,
                    SOLID_TEXEL,
                    SOLID_TEXTURE,
                );
                push_quad(
                    &mut vertices,
                    &mut draws,
                    [w.x, top, w.width, TITLE_BAR_HEIGHT],
                    title,
                    SOLID_TEXEL,
                    SOLID_TEXEL,
                    SOLID_TEXTURE,
                );

                let btn_y = top + (TITLE_BAR_HEIGHT - BUTTON_SIZE) / 2.0;
                let close_x = w.x + w.width - BUTTON_SIZE - BUTTON_MARGIN;
                push_quad(
                    &mut vertices,
                    &mut draws,
                    [close_x, btn_y, BUTTON_SIZE, BUTTON_SIZE],
                    CLOSE_COLOR,
                    SOLID_TEXEL,
                    SOLID_TEXEL,
                    SOLID_TEXTURE,
                );

                let max_x = close_x - BUTTON_SIZE - BUTTON_MARGIN;
                push_quad(
                    &mut vertices,
                    &mut draws,
                    [max_x, btn_y, BUTTON_SIZE, BUTTON_SIZE],
                    MAXIMIZE_COLOR,
                    SOLID_TEXEL,
                    SOLID_TEXEL,
                    SOLID_TEXTURE,
                );

                let min_x = max_x - BUTTON_SIZE - BUTTON_MARGIN;
                push_quad(
                    &mut vertices,
                    &mut draws,
                    [min_x, btn_y, BUTTON_SIZE, BUTTON_SIZE],
                    MINIMIZE_COLOR,
                    SOLID_TEXEL,
                    SOLID_TEXEL,
                    SOLID_TEXTURE,
                );
            }

            let (texture, color, uv0, uv1) = w.look();
            let body = [color[0] * f, color[1] * f, color[2] * f];
            push_quad(
                &mut vertices,
                &mut draws,
                [w.x, w.y, w.width, w.height],
                body,
                uv0,
                uv1,
                texture,
            );
        }

        (vertices, draws)
    }
}

fn push_quad(
    vertices: &mut Vec<Vertex>,
    draws: &mut Vec<DrawCall>,
    rect: [f32; 4],
    color: [f32; 3],
    uv0: [f32; 2],
    uv1: [f32; 2],
    texture: TextureId,
) {
    let first_vertex = vertices.len() as u32;
    push_rect(vertices, rect, color, uv0, uv1);
    draws.push(DrawCall {
        texture,
        first_vertex,
        vertex_count: 6,
    });
}

fn push_rect(
    out: &mut Vec<Vertex>,
    rect: [f32; 4],
    color: [f32; 3],
    uv0: [f32; 2],
    uv1: [f32; 2],
) {
    let [x, y, w, h] = rect;
    let tl = Vertex::textured([x, y], color, [uv0[0], uv0[1]]);
    let tr = Vertex::textured([x + w, y], color, [uv1[0], uv0[1]]);
    let br = Vertex::textured([x + w, y + h], color, [uv1[0], uv1[1]]);
    let bl = Vertex::textured([x, y + h], color, [uv0[0], uv1[1]]);
    out.extend_from_slice(&[tl, tr, br, tl, br, bl]);
}

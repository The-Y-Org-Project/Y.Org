use y_gpu::{DrawCall, SOLID_TEXTURE, TextureId, Vertex};

const SOLID_TEXEL: [f32; 2] = [0.5 / 64.0, 0.5 / 64.0];
const DOCK_CELL: (f32, f32) = (160.0, 100.0);
const DOCK_GAP: f32 = 10.0;
const DOCK_BORDER: f32 = 4.0;
const DOCK_BORDER_COLOR: [f32; 3] = [0.30, 0.35, 0.55];

pub const TITLE_BAR_HEIGHT: f32 = 32.0;
const TITLE_BAR_TOP_COLOR: [f32; 3] = [0.16, 0.18, 0.24];
const TITLE_BAR_BOTTOM_COLOR: [f32; 3] = [0.10, 0.11, 0.15];
const TITLE_BAR_FOCUS_BORDER: [f32; 3] = [0.25, 0.45, 0.75];
const CLOSE_BUTTON_BG: [f32; 3] = [0.18, 0.10, 0.12];
const CLOSE_BUTTON_BG_HOVER: [f32; 3] = [0.85, 0.25, 0.25];
const CLOSE_BUTTON_X: [f32; 3] = [0.85, 0.85, 0.90];
const CLOSE_BUTTON_SIZE: f32 = 20.0;
const CLOSE_BUTTON_MARGIN: f32 = 6.0;
const CLOSE_BUTTON_RADIUS: f32 = 4.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Decorations {
    Server,
    Client,
}

#[derive(Clone, Copy)]
pub struct Restore {
    pub x: f32,
    pub y: f32,
    pub width: i32,
    pub height: i32,
}

pub struct Window {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: [f32; 3],
    pub texture: Option<TextureId>,
    pub maximized: bool,
    pub minimized: bool,
    pub restore: Option<Restore>,
    pub decorations: Decorations,
    pub focused: bool,
}

#[derive(Default)]
pub struct WindowStack {
    pub windows: Vec<Window>,
    next_id: u32,
}

impl WindowStack {
    pub fn add(&mut self, x: f32, y: f32, width: f32, height: f32, color: [f32; 3]) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.windows.push(Window {
            id,
            x,
            y,
            width,
            height,
            color,
            texture: None,
            maximized: false,
            minimized: false,
            restore: None,
            decorations: Decorations::Server,
            focused: false,
        });
        id
    }

    pub fn remove(&mut self, id: u32) {
        self.windows.retain(|w| w.id != id);
    }

    pub fn raise(&mut self, id: u32) {
        if let Some(index) = self.windows.iter().position(|w| w.id == id) {
            let window = self.windows.remove(index);
            self.windows.push(window);
        }
    }

    pub fn window(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn window_mut(&mut self, id: u32) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn set_csd(&mut self, id: u32, csd: bool) {
        if let Some(w) = self.window_mut(id) {
            let new = if csd { Decorations::Client } else { Decorations::Server };
            if w.decorations != new {
                w.decorations = new;
            }
        }
    }

    pub fn set_focused(&mut self, id: Option<u32>) {
        for w in &mut self.windows {
            w.focused = Some(w.id) == id;
        }
    }

    pub fn title_bar_rect(&self, id: u32) -> Option<[f32; 4]> {
        let w = self.window(id)?;
        if w.decorations != Decorations::Server || w.minimized {
            return None;
        }
        Some([w.x, w.y - TITLE_BAR_HEIGHT, w.width, TITLE_BAR_HEIGHT])
    }

    pub fn close_button_rect(&self, id: u32) -> Option<[f32; 4]> {
        let bar = self.title_bar_rect(id)?;
        let x = bar[0] + bar[2] - CLOSE_BUTTON_SIZE - CLOSE_BUTTON_MARGIN;
        let y = bar[1] + (TITLE_BAR_HEIGHT - CLOSE_BUTTON_SIZE) / 2.0;
        Some([x, y, CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE])
    }

    pub fn hit_test(&self, px: f32, py: f32) -> Option<(u32, bool)> {
        self.windows
            .iter()
            .rev()
            .filter(|w| !w.minimized)
            .find(|w| {
                let top = if w.decorations == Decorations::Server {
                    w.y - TITLE_BAR_HEIGHT
                } else {
                    w.y
                };
                let height = if w.decorations == Decorations::Server {
                    w.height + TITLE_BAR_HEIGHT
                } else {
                    w.height
                };
                px >= w.x && px < w.x + w.width && py >= top && py < top + height
            })
            .map(|w| {
                let on_bar = w.decorations == Decorations::Server
                    && py >= w.y - TITLE_BAR_HEIGHT
                    && py < w.y;
                (w.id, on_bar)
            })
    }

    pub fn hit_close_button(&self, px: f32, py: f32) -> Option<u32> {
        for w in self.windows.iter().rev().filter(|w| !w.minimized) {
            if let Some(r) = self.close_button_rect(w.id) {
                if px >= r[0] && px < r[0] + r[2] && py >= r[1] && py < r[1] + r[3] {
                    return Some(w.id);
                }
            }
        }
        None
    }

    pub fn position(&self, id: u32) -> Option<(f32, f32)> {
        self.window(id).map(|w| (w.x, w.y))
    }

    pub fn move_to(&mut self, id: u32, x: f32, y: f32) {
        if let Some(w) = self.window_mut(id) {
            w.x = x;
            w.y = y;
        }
    }

    pub fn set_geometry(&mut self, id: u32, x: f32, y: f32, width: f32, height: f32) {
        if let Some(w) = self.window_mut(id) {
            w.x = x;
            w.y = y;
            w.width = width;
            w.height = height;
        }
    }

    pub fn texture_of(&self, id: u32) -> Option<TextureId> {
        self.window(id).and_then(|w| w.texture)
    }

    pub fn set_body_size(&mut self, id: u32, width: u32, height: u32) -> bool {
        match self.window_mut(id) {
            Some(w) => {
                let new_width = width as f32;
                let new_height = height as f32;
                let changed = w.width != new_width || w.height != new_height;
                w.width = new_width;
                w.height = new_height;
                changed
            }
            None => false,
        }
    }

    pub fn set_texture(&mut self, id: u32, texture: TextureId) {
        if let Some(w) = self.window_mut(id) {
            w.texture = Some(texture);
        }
    }

    pub fn dock_slots(&self, screen: (f32, f32)) -> Vec<(u32, [f32; 4])> {
        let y = screen.1 - DOCK_CELL.1 - DOCK_GAP;
        self.windows
            .iter()
            .filter(|w| w.minimized)
            .enumerate()
            .map(|(i, w)| {
                let x = DOCK_GAP + i as f32 * (DOCK_CELL.0 + DOCK_GAP);
                (w.id, [x, y, DOCK_CELL.0, DOCK_CELL.1])
            })
            .collect()
    }

    pub fn dock_hit(&self, px: f32, py: f32, screen: (f32, f32)) -> Option<u32> {
        self.dock_slots(screen)
            .into_iter()
            .find(|(_, r)| px >= r[0] && px < r[0] + r[2] && py >= r[1] && py < r[1] + r[3])
            .map(|(id, _)| id)
    }

    pub fn scene(&self, screen: (f32, f32)) -> (Vec<Vertex>, Vec<DrawCall>) {
        let mut vertices = Vec::new();
        let mut draws = Vec::new();

        for w in self.windows.iter().filter(|w| !w.minimized) {
            if w.decorations == Decorations::Server {
                push_quad(
                    &mut vertices,
                    &mut draws,
                    [w.x, w.y - TITLE_BAR_HEIGHT + 2.0, w.width, 4.0],
                    [0.0, 0.0, 0.0],
                    SOLID_TEXEL,
                    SOLID_TEXEL,
                    SOLID_TEXTURE,
                );

                push_quad(
                    &mut vertices,
                    &mut draws,
                    [w.x, w.y - TITLE_BAR_HEIGHT, w.width, TITLE_BAR_HEIGHT / 2.0],
                    TITLE_BAR_TOP_COLOR,
                    SOLID_TEXEL,
                    SOLID_TEXEL,
                    SOLID_TEXTURE,
                );
                push_quad(
                    &mut vertices,
                    &mut draws,
                    [w.x, w.y - TITLE_BAR_HEIGHT / 2.0, w.width, TITLE_BAR_HEIGHT / 2.0],
                    TITLE_BAR_BOTTOM_COLOR,
                    SOLID_TEXEL,
                    SOLID_TEXEL,
                    SOLID_TEXTURE,
                );

                if w.focused {
                    push_quad(
                        &mut vertices,
                        &mut draws,
                        [w.x, w.y - TITLE_BAR_HEIGHT, w.width, 1.0],
                        TITLE_BAR_FOCUS_BORDER,
                        SOLID_TEXEL,
                        SOLID_TEXEL,
                        SOLID_TEXTURE,
                    );
                }

                if let Some(btn) = self.close_button_rect(w.id) {
                    push_rounded_rect(
                        &mut vertices,
                        &mut draws,
                        btn,
                        CLOSE_BUTTON_BG,
                        CLOSE_BUTTON_RADIUS,
                    );
                    let cx = btn[0] + btn[2] / 2.0;
                    let cy = btn[1] + btn[3] / 2.0;
                    let s = btn[2] * 0.25;
                    push_line(&mut vertices, &mut draws, cx - s, cy - s, cx + s, cy + s, CLOSE_BUTTON_X);
                    push_line(&mut vertices, &mut draws, cx - s, cy + s, cx + s, cy - s, CLOSE_BUTTON_X);
                }
            }

            let (texture, color, uv0, uv1) = surface_look(w);
            push_quad(
                &mut vertices,
                &mut draws,
                [w.x, w.y, w.width, w.height],
                color,
                uv0,
                uv1,
                texture,
            );
        }

        for (id, cell) in self.dock_slots(screen) {
            push_quad(
                &mut vertices,
                &mut draws,
                [
                    cell[0] - DOCK_BORDER,
                    cell[1] - DOCK_BORDER,
                    cell[2] + DOCK_BORDER * 2.0,
                    cell[3] + DOCK_BORDER * 2.0,
                ],
                DOCK_BORDER_COLOR,
                SOLID_TEXEL,
                SOLID_TEXEL,
                SOLID_TEXTURE,
            );

            let Some(w) = self.window(id) else {
                continue;
            };
            let (texture, color, uv0, uv1) = surface_look(w);
            let scale = if w.width > 0.0 && w.height > 0.0 {
                (cell[2] / w.width).min(cell[3] / w.height)
            } else {
                1.0
            };
            let thumb_width = w.width * scale;
            let thumb_height = w.height * scale;
            push_quad(
                &mut vertices,
                &mut draws,
                [
                    cell[0] + (cell[2] - thumb_width) / 2.0,
                    cell[1] + (cell[3] - thumb_height) / 2.0,
                    thumb_width,
                    thumb_height,
                ],
                color,
                uv0,
                uv1,
                texture,
            );
        }

        (vertices, draws)
    }
}

fn surface_look(w: &Window) -> (TextureId, [f32; 3], [f32; 2], [f32; 2]) {
    match w.texture {
        Some(t) => (t, [1.0, 1.0, 1.0], [0.0, 0.0], [1.0, 1.0]),
        None => (SOLID_TEXTURE, w.color, SOLID_TEXEL, SOLID_TEXEL),
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

fn push_rect(out: &mut Vec<Vertex>, rect: [f32; 4], color: [f32; 3], uv0: [f32; 2], uv1: [f32; 2]) {
    let [x, y, w, h] = rect;
    let tl = Vertex::textured([x, y], color, [uv0[0], uv0[1]]);
    let tr = Vertex::textured([x + w, y], color, [uv1[0], uv0[1]]);
    let br = Vertex::textured([x + w, y + h], color, [uv1[0], uv1[1]]);
    let bl = Vertex::textured([x, y + h], color, [uv0[0], uv1[1]]);
    out.extend_from_slice(&[tl, tr, br, tl, br, bl]);
}

fn push_rounded_rect(
    vertices: &mut Vec<Vertex>,
    draws: &mut Vec<DrawCall>,
    rect: [f32; 4],
    color: [f32; 3],
    radius: f32,
) {
    let [x, y, w, h] = rect;
    let r = radius.min(w / 2.0).min(h / 2.0);
    let mut verts = Vec::new();
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let pts = [
        [x + r, y],
        [x + w - r, y],
        [x + w, y + r],
        [x + w, y + h - r],
        [x + w - r, y + h],
        [x + r, y + h],
        [x, y + h - r],
        [x, y + r],
    ];
    for i in 0..8 {
        let p1 = pts[i];
        let p2 = pts[(i + 1) % 8];
        verts.extend_from_slice(&[
            Vertex::textured([cx, cy], color, SOLID_TEXEL),
            Vertex::textured(p1, color, SOLID_TEXEL),
            Vertex::textured(p2, color, SOLID_TEXEL),
        ]);
    }
    let first_vertex = vertices.len() as u32;
    vertices.extend_from_slice(&verts);
    draws.push(DrawCall {
        texture: SOLID_TEXTURE,
        first_vertex,
        vertex_count: verts.len() as u32,
    });
}

fn push_line(
    vertices: &mut Vec<Vertex>,
    draws: &mut Vec<DrawCall>,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: [f32; 3],
) {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 {
        return;
    }
    let nx = -dy / len * 1.0;
    let ny = dx / len * 1.0;
    let p1 = [x1 + nx, y1 + ny];
    let p2 = [x2 + nx, y2 + ny];
    let p3 = [x2 - nx, y2 - ny];
    let p4 = [x1 - nx, y1 - ny];
    let first_vertex = vertices.len() as u32;
    push_rect(vertices, [p1[0], p1[1], p3[0] - p1[0], p3[1] - p1[1]], color, SOLID_TEXEL, SOLID_TEXEL);
    draws.push(DrawCall {
        texture: SOLID_TEXTURE,
        first_vertex,
        vertex_count: 6,
    });
}

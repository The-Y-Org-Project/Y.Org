use y_desktop::TITLE_BAR_HEIGHT;
use y_gpu::{DrawCall, SOLID_TEXTURE, TextureId, Vertex};

const SOLID_TEXEL: [f32; 2] = [0.5 / 64.0, 0.5 / 64.0];

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
    pub has_title_bar: bool,
    pub restore: Option<Restore>,
}

impl Window {
    pub fn info(&self, focused: bool) -> y_desktop::WindowInfo {
        y_desktop::WindowInfo {
            id: self.id,
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            color: self.color,
            texture: self.texture,
            minimized: self.minimized,
            maximized: self.maximized,
            has_title_bar: self.has_title_bar,
            focused,
        }
    }
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
            has_title_bar: true,
            restore: None,
        });
        id
    }

    pub fn infos(&self, focused: Option<u32>) -> Vec<y_desktop::WindowInfo> {
        self.windows
            .iter()
            .map(|w| w.info(focused == Some(w.id)))
            .collect()
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

    pub fn lower(&mut self, id: u32) {
        if let Some(index) = self.windows.iter().position(|w| w.id == id) {
            let window = self.windows.remove(index);
            self.windows.insert(0, window);
        }
    }

    pub fn window(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn window_mut(&mut self, id: u32) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn hit_test(&self, px: f32, py: f32) -> Option<u32> {
        self.windows
            .iter()
            .rev()
            .filter(|w| !w.minimized)
            .find(|w| {
                let top_y = if w.has_title_bar {
                    w.y - TITLE_BAR_HEIGHT
                } else {
                    w.y
                };
                px >= w.x && px < w.x + w.width && py >= top_y && py < w.y + w.height
            })
            .map(|w| w.id)
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

    pub fn size_of(&self, id: u32) -> Option<(f32, f32)> {
        self.window(id).map(|w| (w.width, w.height))
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

    pub fn scene(&self) -> (Vec<Vertex>, Vec<DrawCall>) {
        let mut vertices = Vec::new();
        let mut draws = Vec::new();

        for w in self.windows.iter().filter(|w| !w.minimized) {
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

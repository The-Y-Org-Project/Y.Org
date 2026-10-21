use y_gpu::{DrawCall, SOLID_TEXTURE, TextureId, Vertex};

const TITLE_BAR_HEIGHT: f32 = 28.0;
const TITLE_BAR_SHADE: f32 = 0.55;
const SOLID_TEXEL: [f32; 2] = [0.5 / 64.0, 0.5 / 64.0];

pub struct Window {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: [f32; 3],
    pub texture: Option<TextureId>,
}

#[derive(Default)]
pub struct WindowStack {
    windows: Vec<Window>,
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

    pub fn hit_test(&self, px: f32, py: f32) -> Option<(u32, bool)> {
        self.windows
            .iter()
            .rev()
            .find(|w| px >= w.x && px < w.x + w.width && py >= w.y && py < w.y + w.height)
            .map(|w| (w.id, py < w.y + TITLE_BAR_HEIGHT))
    }

    pub fn position(&self, id: u32) -> Option<(f32, f32)> {
        self.windows.iter().find(|w| w.id == id).map(|w| (w.x, w.y))
    }

    pub fn move_to(&mut self, id: u32, x: f32, y: f32) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.x = x;
            w.y = y;
        }
    }

    pub fn pending_textures(&self) -> Vec<(u32, u32, u32)> {
        self.windows
            .iter()
            .filter(|w| w.texture.is_none())
            .map(|w| (w.id, w.width as u32, (w.height - TITLE_BAR_HEIGHT) as u32))
            .collect()
    }

    pub fn texture_of(&self, id: u32) -> Option<TextureId> {
        self.windows
            .iter()
            .find(|w| w.id == id)
            .and_then(|w| w.texture)
    }

    pub fn set_body_size(&mut self, id: u32, width: u32, height: u32) -> bool {
        match self.windows.iter_mut().find(|w| w.id == id) {
            Some(w) => {
                let new_width = width as f32;
                let new_height = height as f32 + TITLE_BAR_HEIGHT;
                let changed = w.width != new_width || w.height != new_height;
                w.width = new_width;
                w.height = new_height;
                changed
            }
            None => false,
        }
    }

    pub fn set_texture(&mut self, id: u32, texture: TextureId) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.texture = Some(texture);
        }
    }

    pub fn scene(&self) -> (Vec<Vertex>, Vec<DrawCall>) {
        let mut vertices = Vec::with_capacity(self.windows.len() * 12);
        let mut draws = Vec::with_capacity(self.windows.len() * 2);

        for w in &self.windows {
            let title_color = [
                w.color[0] * TITLE_BAR_SHADE,
                w.color[1] * TITLE_BAR_SHADE,
                w.color[2] * TITLE_BAR_SHADE,
            ];
            let first_vertex = vertices.len() as u32;
            push_rect(
                &mut vertices,
                [w.x, w.y, w.width, TITLE_BAR_HEIGHT],
                title_color,
                SOLID_TEXEL,
                SOLID_TEXEL,
            );
            draws.push(DrawCall {
                texture: SOLID_TEXTURE,
                first_vertex,
                vertex_count: 6,
            });

            let (texture, color, uv0, uv1) = match w.texture {
                Some(t) => (t, [1.0, 1.0, 1.0], [0.0, 0.0], [1.0, 1.0]),
                None => (SOLID_TEXTURE, w.color, SOLID_TEXEL, SOLID_TEXEL),
            };
            let first_vertex = vertices.len() as u32;
            push_rect(
                &mut vertices,
                [
                    w.x,
                    w.y + TITLE_BAR_HEIGHT,
                    w.width,
                    w.height - TITLE_BAR_HEIGHT,
                ],
                color,
                uv0,
                uv1,
            );
            draws.push(DrawCall {
                texture,
                first_vertex,
                vertex_count: 6,
            });
        }

        (vertices, draws)
    }
}

fn push_rect(out: &mut Vec<Vertex>, rect: [f32; 4], color: [f32; 3], uv0: [f32; 2], uv1: [f32; 2]) {
    let [x, y, w, h] = rect;
    let tl = Vertex::textured([x, y], color, [uv0[0], uv0[1]]);
    let tr = Vertex::textured([x + w, y], color, [uv1[0], uv0[1]]);
    let br = Vertex::textured([x + w, y + h], color, [uv1[0], uv1[1]]);
    let bl = Vertex::textured([x, y + h], color, [uv0[0], uv1[1]]);
    out.extend_from_slice(&[tl, tr, br, tl, br, bl]);
}

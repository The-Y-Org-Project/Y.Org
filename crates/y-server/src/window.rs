use y_gpu::Vertex;

const TITLE_BAR_HEIGHT: f32 = 28.0;
const TITLE_BAR_SHADE: f32 = 0.55;
const TEXTURE_SIZE: f32 = 64.0;
const SOLID_TEXEL: [f32; 2] = [0.5 / TEXTURE_SIZE, 0.5 / TEXTURE_SIZE];

pub struct Window {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: [f32; 3],
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
        });
        id
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

    pub fn vertices(&self) -> Vec<Vertex> {
        let mut out = Vec::with_capacity(self.windows.len() * 12);

        for w in &self.windows {
            let title_color = [
                w.color[0] * TITLE_BAR_SHADE,
                w.color[1] * TITLE_BAR_SHADE,
                w.color[2] * TITLE_BAR_SHADE,
            ];
            push_rect(
                &mut out,
                [w.x, w.y, w.width, TITLE_BAR_HEIGHT],
                title_color,
                SOLID_TEXEL,
                SOLID_TEXEL,
            );

            let body_height = w.height - TITLE_BAR_HEIGHT;
            push_rect(
                &mut out,
                [w.x, w.y + TITLE_BAR_HEIGHT, w.width, body_height],
                w.color,
                [0.0, 0.0],
                [w.width / TEXTURE_SIZE, body_height / TEXTURE_SIZE],
            );
        }

        out
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

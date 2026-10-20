#version 450

layout(push_constant) uniform Screen {
    vec2 resolution;
} screen;

layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec3 in_color;

layout(location = 0) out vec3 frag_color;

void main() {
    vec2 ndc = in_pos / screen.resolution * 2.0 - 1.0;
    gl_Position = vec4(ndc, 0.0, 1.0);
    frag_color = in_color;
}

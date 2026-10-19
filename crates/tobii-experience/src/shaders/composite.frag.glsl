// GazeNative.dll's composite pixel shader (blob 2), as docs/gaze-preview.md transcribes it.
// The output is premultiplied, as GTK expects from a GLArea with an alpha channel.
precision highp float;
layout(std140) uniform Params {
    vec4 gaze, gaze1, gaze2, aspect, decay, strength, radius, color, trail, heatmap, background_color;
};
uniform highp sampler2D accumulation;
uniform sampler2D gradient;
in vec2 uv;
out vec4 frag;
void main() {
    float v = texture(accumulation, uv).x;
    float h = heatmap.x;
    float u = clamp(h * v + v * (1.0 - h) * 0.13, 0.0, 1.0);
    vec4 g = texture(gradient, vec2(u, 0.0));
    float a = clamp(g.a, 0.0, 1.0);
    vec4 c = (background_color * (1.0 - a) + color * a) * (1.0 - h);
    c = h * vec4(g.rgb, g.a * color.a) + c;
    frag = vec4(c.rgb * c.a, c.a);
}

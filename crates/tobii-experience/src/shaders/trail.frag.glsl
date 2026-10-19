// GazeNative.dll's trail pixel shader (blob 4), as docs/gaze-preview.md transcribes it.
precision highp float;
layout(std140) uniform Params {
    vec4 gaze, gaze1, gaze2, aspect, decay, strength, radius, color, trail, heatmap, background_color;
};
uniform highp sampler2D previous;
in vec2 uv;
out vec4 frag;
void main() {
    vec2 s = vec2(aspect.x, 1.0);
    vec2 q = uv * s;
    float f0 = 1.0 - clamp(length(gaze.xy * s - q) / radius.x, 0.0, 1.0);
    float c1 = clamp(length(gaze1.xy * s - q) / radius.x, 0.0, 1.0);
    float c2 = clamp(length(gaze2.xy * s - q) / radius.x, 0.0, 1.0);
    float splat = (((f0 - c1) + 1.0) - c2 + 1.0) * strength.x;
    float last = texture(previous, uv).x;
    float d0 = length(q - gaze.xy * s);
    float outside = clamp(d0 - radius.x, 0.0, 1.0);
    float present = d0 < aspect.x ? 1.0 : 0.0;
    float keep = clamp(decay.x - outside * (present * (1.0 - trail.x)) * 5.0, 0.0, 1.0);
    frag = vec4(last * keep + splat, 0.0, 0.0, 1.0);
}

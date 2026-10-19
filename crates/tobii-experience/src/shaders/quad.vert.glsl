// GazeNative.dll's vertex shader (blobs 1, 3, 5) for GL's bottom-left origin: the quad is
// generated from gl_VertexID and uv equals the position, so every pass keeps one convention.
// The #version line is prepended for the context (300 es or 150).
out vec2 uv;
void main() {
    vec2 corners[6] = vec2[6](vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
                              vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    vec2 position = corners[gl_VertexID];
    gl_Position = vec4(position * 2.0 - 1.0, 1.0, 1.0);
    uv = position;
}

# Gaze preview: GazeNative.dll

How Tobii Experience draws "Preview my gaze": a click-through, always-on-top
window over the tracked monitor that shows a fading bubble where the user
looks. The managed side forwards gaze points; native `GazeNative.dll` smooths
them on the CPU and draws them with Direct3D 11 into two quarter-resolution
float accumulation textures, then composites one of them onto the window. This
ledger gives the exact algorithm, constants, resource formats, states and the
six shaders, each with a GLSL ES 3.0 equivalent, for the Linux GTK overlay.

Binary: `evidence/private/experience/portal/app/Tobii.EyeTracking.Portal.WPF/GazeNative.dll`
(the Store app's copy, 59 816 bytes, x64, image base `0x180000000`), SHA-256
`790c88ff69275893fcea9f5291cfdfd1d2e4a7dabf740cea1a354f29727d39f4`. The MSI
copy (`evidence/private/experience/msi/GazeNative.dll.F15EE609_...`, SHA-256
`472b22958beeb0395657f43aa52ae0aec4963a16b7a5016c7a3c009305a12562`) has the
same size and section layout; it differs only in the DOS stub/rich header
(PE header 8 bytes earlier), the debug directory timestamps and PDB GUID
(file offsets `0x5f94` to `0x606f`) and the Authenticode signature after
`0xc400`. `.text`, the shaders and `.data` are byte-identical, so everything
below holds for both (static).

Evidence labels:

- **static**: read from the binary (Ghidra 12.1.4 through REA, `objdump -d`
  for instruction-level checks) or from the decompiled managed callers under
  `evidence/private/experience/decompiled/` and
  `evidence/private/experience/portal/decompiled/`.
- **disassembly**: the DXBC blobs written by `tools/gaze_native_shaders.py`
  to `evidence/private/experience/shaders/` and disassembled to `*.asm`
  beside them (see [Shaders](#shaders) for the disassembler).
- **inference**: reasoning not checked directly. Nothing here was observed
  live; the overlay was not run.

## Managed side

`Tobii.Tech.NETCommon.TrackingDevice.GazeTrace.GazeTraceVisualization`
(`decompiled/Tobii.Tech.NETCommon.TrackingDevice.dll/.../GazeTraceVisualization.cs`),
static:

- The constructor calls `GazeNative_Initialize` (:176), then on a worker task
  sets Size 0.075, Strength 0.95, Decay 0.95, Filter 0.75, Trail 0.7 and
  `ForegroundColor = Color.Silver` (:189-194). `Color.Silver` is packed
  `0xFFC0C0C0` (`Tobii.Tech.NETCommon.Mathematics.Color`), so the colour is
  (192, 192, 192, 255) / 255 = (0.75294, 0.75294, 0.75294, 1.0) through
  `ColorExtensions.ToFloat` (byte / 255). The background colour is never set.
- It then loops: `GazeNative_Update`, then sleeps until 12 ms after the start
  of the tick when enabled, 500 ms when not (:197-213). `Task.Delay` granularity
  on Windows is the system timer, so the real tick may be longer than 12 ms
  (inference, not measured).
- Visibility (:237-256): becoming visible calls `GazeNative_Show` and
  subscribes `FilteredCombinedAdvancedGaze`; becoming hidden calls
  `GazeNative_Hide` and disposes the subscription. The Store app's
  `GazeTraceVisualizationExpApp` (`portal/decompiled/.../GazeTraceVisualizationExpApp.cs`)
  is visible only while enabled and the app is active.
- Each valid sample (:258-271): `GazeNative_GetScreenBounds`, then
  `x = (int)(left + width * gaze.x)`, `y = (int)(top + height * gaze.y)` in
  doubles with truncation, then `GazeNative_AddGazePoint(x, y)`. Invalid
  samples are dropped.
- Screen bounds (:273-286) are the device bounds of the monitor mapped to the
  default tracker, refreshed on display changes, passed to
  `GazeNative_SetScreenBounds`. `Dispose` calls `GazeNative_Stop`.

## Exports and parameters

Every setter stores one global; the getters read it back (static,
`0x180003880` to `0x1800039f0`).

| Setter | Global | `Initialize` default | Managed value | Used by |
| --- | --- | --- | --- | --- |
| `SetSize` | `0x18000de6c` | 0.04 (`0x180006db0`) | 0.075 | CB `radius`; bounding-box radius |
| `SetStrength` | `0x18000de90` | 0.5 (`0x180006db8`) | 0.95 | CB `strength` |
| `SetDecay` | `0x18000e5c0` | 0.35 (`0x180006db4`) | 0.95 | CB `decay` |
| `SetTrail` | `0x18000deec` | 1.0 | 0.7 | CB `trail`; bounding-box history length |
| `SetFilter` | `0x18000df64` | 0.0 | 0.75 | CPU smoothing in `Update` |
| `SetColor` | `0x18000e6b0` (rgba) | (1, 1, 1, 0.5) | Silver | CB `color` |
| `SetBackgroundColor` | `0x18000e6a0` (rgba) | (0, 0, 0, 0) | not set | CB `background_color`; back-buffer clear |
| none | `0x18000df60` (mode) | 0 | none | gradient choice, CB `heatmap`, full-screen box |

The mode at `0x18000df60` is written only by `Initialize` (`0x18000333b`,
zero); every other reference reads it. So in the shipped DLL the mode is
always 0: gradient 0 is used, `heatmap` is 0.0 and the bounding box is
computed. The mode 1 and mode 2 paths below are dead code. The native Size
default is 0.04, not 0.075: the store at `0x1800033dd` loads it from
`0x180006db0`; 0.075 is the managed value.

Other exports: `AddGazePoint(int x, int y)` `0x180003620` stores x and y as
doubles at `0x18000de50`/`0x18000de58` and `QueryPerformanceCounter` at
`0x18000de48`. `SetScreenBounds(x, y, w, h)` `0x180003650` stores left
`0x18000de64`, top `0x18000de60`, width `0x18000de44`, height `0x18000de40`.
`Show` `0x180003580`: `ShowWindow(SW_SHOWNOACTIVATE)`; if the shown flag
`0x18000de94` was 0, clears the history ring and the previous point to
-100000 and sets the flag. `Hide` `0x180003600`: flag 0, `ShowWindow(SW_HIDE)`.
`Stop` `0x180003730` releases every D3D object and hides the window.
`Update` `0x180003720` jumps to `0x180002df0`; `Initialize` `0x180003570`
jumps to `0x180003260`.

## Window and device

`Initialize` `0x180003260` (static):

- Screen bounds start as the desktop window rectangle.
- Window class `GazeTraceWindow` (wndproc `0x180003b30`: `WM_CLOSE` posts
  quit, else `DefWindowProcW`; background `GetStockObject(BLACK_BRUSH)`),
  title "Tobii Streaming Gaze Overlay", extended style `0x8080028`
  (`WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST`),
  style `0x90000000` (`WS_POPUP | WS_VISIBLE`), at (0, 0) with the bounds'
  size. `SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA)`. It is
  click-through and never takes focus.
- `0x180003b60`: `DwmExtendFrameIntoClientArea` with margins (-1, 0, 0, 0),
  so the swap chain's alpha is the window's per-pixel alpha. On failure it
  sets `0x18000e5c4`, moves the window to x = -100000 and shows "Preview mode
  is unavailable. Please make sure you are running DWM (Aero theme)".
- `0x180003a40` (run when the bounds change): `SetWindowPos(HWND_TOPMOST)` to
  the bounds (x = -100000 without DWM), then `ITaskbarList::DeleteTab(hwnd)`
  (CLSID `56FDF344-FD6D-11D0-958A-006097C9A090` at `0x180005250`, IID
  `56FDF342-...` at `0x180005240`, vtable `+0x28`), so it has no taskbar button.
- Every `Update` also calls `SetWindowPos(HWND_TOPMOST, flags 0x71b)` to stay
  on top (skipped only when DWM failed; the other guard, `0x18000de97`, is
  always 0).
- Defaults as in the table, the history ring `0x18000df80` (200 entries of
  float x, y) and the previous point `0x18000de80`/`0x18000de84` set to
  -100000, ring index `0x18000de88` = 0, shown flag `0x18000de94` = 1, the
  "was shown last frame" flag `0x18000de96` = 1, then `0x180001620`, then
  `ShowWindow(SW_HIDE)`.

`0x180001620` creates the D3D11 device (static): `D3D11CreateDeviceAndSwapChain`
with a hardware driver, no flags, SDK 7; swap chain of 2 buffers,
`DXGI_FORMAT_R8G8B8A8_UNORM`, back-buffer size W x H (the bounds, zero
replaced by 1), refresh 0/0, 1 sample, usage render target, windowed,
`DXGI_SWAP_EFFECT_DISCARD`, flag `ALLOW_MODE_SWITCH`. Present flag
`0x18000de95` = 1. Then the accumulation textures (`0x1800010b0`), the
gradient (`0x180001290`), the quad (`0x1800013e0`), the constant buffer, the
six shaders, input layout, sampler, rasterizer and blend state.

## Update: the CPU trail

`0x180002df0`, every managed tick (static; float operations checked in the
disassembly: all single precision except the raw point, which is a double
converted to float, and the time test, which is double):

1. If the bounds changed: `0x180003b60`, `0x180003a40`. If the size changed:
   W = width (`0x18000de68`), H = height (`0x18000deac`), recreate the
   accumulation textures and the quad, `ResizeBuffers(2, W or 1, H or 1)` and
   recreate the back-buffer view.
2. Reset this frame's three new points `0x18000dec0` (`gaze`, `gaze1`,
   `gaze2`, float x, y each) to (-100000, -100000).
3. If `(QPC_now - QPC_last_AddGazePoint) / QPF < 0.1` (double, constant
   `0x180006dc8`) and the shown flag is set:
   ```
   p = (float)raw - (float)(left, top)          // screen-relative pixels
   if prev.x > -100000:                         // 0x180006de0
       t = 1.0 - Filter * 0.9                   // 0x180006dbc, 0x180006dc0
       p = (p - prev) * t + prev
   if prev.x > -100:                            // 0x180006dd4
       d = p - prev
       gaze  = d / 5.0 + prev                   // 0x180006dd0
       gaze1 = (d + d) / 5.0 + prev
       gaze2 = p
   else:
       gaze  = p                                // gaze1, gaze2 stay -100000
   prev = p
   ring[index] = p; index += 1; if index > 199: index = 0
   ```
   Only the x of `prev` is tested. The raw point is the last one passed to
   `AddGazePoint`, so every tick within 0.1 s of a sample smooths towards it
   again: the smoothing is per tick, not per sample. With Filter 0.75,
   t = 0.325 (float32 `0.32500005`). There is no 3/5 or 4/5 point.
4. Render (`0x180002700`).

The history ring holds only the final points and feeds only the bounding box;
the shaders see just `gaze`, `gaze1`, `gaze2` of the current frame.

## Render: `0x180002700`

### Resources

| Object | Global | Format and size | Notes |
| --- | --- | --- | --- |
| Accumulation A | `0x18000df10`, RTV `0x18000df58`, SRV `0x18000df30` | `R32_FLOAT`, W/4 x H/4 | "current" |
| Accumulation B | `0x18000def8`, RTV `0x18000def0`, SRV `0x18000df78` | `R32_FLOAT`, W/4 x H/4 | "previous" |
| Gradient | `0x18000df40`, SRV `0x18000df00` | `R8G8B8A8_UNORM`, 1536 x 1 | immutable data, see [Gradient](#gradient) |
| Quad | `0x18000dee0` | 96 bytes, dynamic | 6 vertices, stride 16 |
| Constant buffer | `0x18000dea0` | 176 bytes, dynamic | see [Constant buffer](#constant-buffer) |
| Back buffer RTV | `0x18000df20` | `R8G8B8A8_UNORM`, W x H | swap chain buffer 0 |

W/4 and H/4 are `(n + (n >> 31 & 3)) >> 2` (division truncating towards zero),
each replaced by 1 when zero (`0x1800010b0`). Textures: 1 mip, 1 sample,
default usage, bind render target and shader resource, no initial data; the
render target views are `R32_FLOAT` `TEXTURE2D` mip 0. The quarter textures
are not initialised (D3D gives undefined contents; inference: drivers return
zeros); they are cleared only when the overlay hides.

The quad (`0x1800013e0`) holds float2 position and float2 texcoord, equal to
each other: (0,1) (0,0) (1,0), (0,1) (1,0) (1,1). Input layout `POSITION`
`R32G32_FLOAT` at 0 and `TEXCOORD` `R32G32_FLOAT` at 8, slot 0. Topology
triangle list, `Draw(6, 0)`.

States (all static, from the descriptors in `0x180001620`):

- Rasterizer (`0x18000deb0`, set once): solid fill, cull back, front faces
  clockwise, depth clip on, **scissor on**, no multisample. Both quad
  triangles are clockwise on screen after the vertex shader, so none is culled.
- Sampler (`0x18000df38`): `MIN_MAG_MIP_LINEAR`, address clamp on U, V, W,
  LOD bias 0, max anisotropy 0, comparison never-set (0), border 0, MinLOD 0,
  MaxLOD 0.
- Blend for the trail and clear passes: none (`OMSetBlendState(NULL)`, so
  blending off, write mask all).
- Blend for the composite (`0x18000de70`): enabled, colour ONE x src + ZERO x
  dst, ADD; alpha ONE, ZERO, ADD; write mask 0xF; set with a null blend factor
  and sample mask `0x0fffffff`. In effect the composite replaces the pixel.
- Viewports have MinDepth 0 and MaxDepth 0; there is no depth buffer. The
  vertex shader emits z = w = 1, inside the depth clip range.

### Passes while shown

Shaders are named by their blob index (next section). Wq = W/4, Hq = H/4.

1. Write the constant buffer (`0x1800023f0`, `Map` `WRITE_DISCARD`).
2. Bind the quad and topology. Bounding box `L, T, R, B` (full-resolution
   pixels) from `0x180001fc0`. Scissor `(L/4, T/4, R/4, B/4)` (same
   truncating division), viewport `(0, 0, Wq, Hq)`.
3. Only if the mode changed (never, see above): rebuild the gradient and clear
   B with the clear pass below over the whole quarter target.
4. **Trail pass**: render target A, blending off, VS 3, PS 4, constant buffer
   on VS and PS slot 0, `t0` = B (previous), sampler `s0`, draw.
5. **Clear outside the box** (`0x180001b80`): render target A, VS 5, PS 0 (writes
   0), same viewport, four draws with scissors left `(0, 0, L/4, Hq)`, right
   `(R/4, 0, Wq, Hq)`, top `(L/4, 0, R/4, T/4)`, bottom `(L/4, B/4, R/4, Hq)`.
   A is now fully written.
6. Swap A and B (textures, render target views and shader resource views).
   B now holds this frame's trail.
7. **Composite**: clear the back buffer to the background colour
   premultiplied, `(r*a, g*a, b*a, a)` (default all zero, transparent);
   viewport `(0, 0, W, H)` (zero replaced by 1); scissor `(L, T, R, B)` at full
   resolution; composite blend; render target the back buffer; VS 1, PS 2;
   `t0` = B, `t1` = gradient; sampler `s0`; draw. Outside the box the window
   stays the clear colour.
8. `Present(0, 0)` (no vsync) when `0x18000de95` is set (always after init).

### Passes on the first hidden frame

When the shown flag is 0 and the "was shown" flag is 1 (the frame after
`Hide`), static: write the constant buffer; scissor `(0, 0, Wq, Hq)` and
viewport `(0, 0, Wq, Hq)`; clear B (`0x180001e30`: render target B, VS 5, PS 0,
`t0` = A bound but unused, full quarter scissor); clear the back buffer to
(0, 0, 0, 0) with the full viewport and no scissor rectangle (`0x180002620`);
`Present`; clear it again. Later hidden frames draw nothing. The "was shown"
flag takes the shown flag's value at the end of every frame.

### Bounding box: `0x180001fc0`

Static. Mode 2 would return the whole target; mode 0:

```
box = (L, T, R, B) = (W, H, 0, 0)                    // ints, inverted
aspect = W / H  (1.0 when H == 0)
n = 60 - (int)(Trail * -140.0)                      // float mul, truncation
n = min(n, 200)                                     // Trail 0.7 gives 158
r = W * Size * 1.1                                  // float, 0x180006dc4
if r <= r_prev: r = (r - r_prev) * 0.02 + r_prev     // 0x180006dac; r_prev 0x18000dea8
r_prev = r                                           // r_prev starts at 0
i = index - 1
repeat n times (if n >= 1):
    if i < 0: i = 199
    (x, y) = ring[i]
    if x > -10000:                                   // 0x180006ddc
        if x - r/aspect < L: L = (int)(x - r/aspect)
        if x + r/aspect > R: R = (int)(x + r/aspect)
        if y - r < T: T = (int)(y - r)
        if y + r > B: B = (int)(y + r)
    i -= 1
L = max(L, 0); R = min(R, W); T = max(T, 0); B = min(B, H)
```

So the box covers the last n final points (158 ticks, about 1.9 s at 12 ms)
padded by `H * Size * 1.1` horizontally and `W * Size * 1.1` vertically. The
splat radius in the shader is `Size * H` pixels in both directions, so the
horizontal pad is the radius plus 10 % and the vertical pad is larger than
needed. The pad grows at once and shrinks by 2 % of the difference per frame.
With no points the box is empty (L = W, R = 0): the trail pass is fully
scissored out and the left and right clear strips cover the whole target.

## Constant buffer

`0x1800023f0` fills 176 bytes; the names and offsets are the shaders' own
reflection data (`RDEF`, every member a `float4`), static and disassembly:

| Offset | Name | Value written |
| --- | --- | --- |
| 0x00 | `gaze` | (gaze.x / W, gaze.y / H, 0, 0) |
| 0x10 | `gaze1` | (gaze1.x / W, gaze1.y / H, 0, 0) |
| 0x20 | `gaze2` | (gaze2.x / W, gaze2.y / H, 0, 0) |
| 0x30 | `aspect` | (W / H, 0, 0, 0); 1.0 when H == 0 |
| 0x40 | `decay` | (Decay, 0, 0, 0) |
| 0x50 | `strength` | (Strength, 0, 0, 0) |
| 0x60 | `radius` | (Size, 0, 0, 0) |
| 0x70 | `color` | Color rgba |
| 0x80 | `trail` | (Trail, 0, 0, 0) |
| 0x90 | `heatmap` | (mode == 2 ? 1.0 : 0.0, 0, 0, 0), always 0.0 |
| 0xa0 | `background_color` | BackgroundColor rgba |

W and H are the back-buffer size in pixels (the bounds). Unused points are
-100000 / W and -100000 / H, far outside the screen. The same buffer is bound
to VS and PS slot 0 in every pass. In GLSL ES 3.0 a `std140` uniform block of
eleven `vec4` members has exactly this layout, so the 176 bytes can be
uploaded unchanged.

## Shaders

`tools/gaze_native_shaders.py` finds the six `DXBC` containers in `.rdata`
(static and disassembly). All are shader model 4.0, compiled by "Microsoft (R)
HLSL Shader Compiler 9.29.952.3111". Their addresses are the ones passed to
`CreateVertexShader`/`CreatePixelShader` in `0x180001620`:

| Blob | Address | Size | SHA-256 | Created as | Role |
| --- | --- | --- | --- | --- | --- |
| `0-ps` | `0x180005530` | 0x1d4 | `e232bf11...4300fe9` | PS `0x18000df48` | clear |
| `1-vs` | `0x180005710` | 0x26c | `33ce48a9...6c2a13f` | VS `0x18000ded8`, input layout | composite |
| `2-ps` | `0x180005980` | 0x610 | `ca358771...6cf09fb0` | PS `0x18000df08` | composite |
| `3-vs` | `0x180005f90` | 0x26c | `33ce48a9...6c2a13f` | VS `0x18000deb8` | trail |
| `4-ps` | `0x180006200` | 0x82c | `132a4348...b89547b3` | PS `0x18000df70` | trail |
| `5-vs` | `0x180006a30` | 0x26c | `33ce48a9...6c2a13f` | VS `0x18000de78` | clear |

Full digests: `0-ps` `e232bf111c282f642281102dfe0a6775eac837ef43d2172d60bd1634b4300fe9`,
VS `33ce48a983d2386770c5e3f3bac4b139e4741f8b83f8922d6c9e8cd0e6c2a13f` (the three
vertex shaders are byte-identical), `2-ps`
`ca35877140845b2f453550a8651582f489b486c055f54231c1ed654d6cf09fb0`, `4-ps`
`132a4348a84aff7f398f24323da28b798bc18c37b3b20b0c74fca25bb89547b3`.

Disassembly: vkd3d-compiler 1.15 (vkd3d-shader 1.15, `nixpkgs` release-25.05
build `/nix/store/w056kq3vwmccafj4cnif14lzpzhvr8gw-vkd3d-1.15`, run through
nix-portable), `vkd3d-compiler -x dxbc-tpf -b d3d-asm` (1.15 spells the target
option `-b`, not `-y`). The current `nixpkgs#vkd3d` (2.1) is not in the binary
cache and would have built the whole toolchain from source, so the cached 1.15
store path was used. A small token decoder written for this check gave the same
instructions, operands and immediates for all six blobs. The listings below are
vkd3d's output unchanged.

The GLSL ES 3.0 shaders below compile with glslangValidator 15.3.0, and a
Python transcription of the trail and composite listings gives the same result
as the GLSL formulas on 20 000 random inputs (largest difference 1.4e-14), so
the rewrites keep the operation order and constants of the originals.

### Vertex shader (blobs 1, 3, 5)

```
vs_4_0
dcl_input v0.xy
dcl_input v1.xy
dcl_output_siv o0.xyzw, position
dcl_output o1.xy
mad o0.xy, v0.xyxx, l(2.00000000e+00, -2.00000000e+00, 0.00000000e+00, 0.00000000e+00), l(-1.00000000e+00, 1.00000000e+00, 0.00000000e+00, 0.00000000e+00)
mov o0.zw, l(0, 0, 1.00000000e+00, 1.00000000e+00)
mov o1.xy, v1.xyxx
ret
```

Position (x, y) in [0, 1] maps to clip `(2x - 1, 1 - 2y, 1, 1)`; the texcoord
passes through, so texcoord (0, 0) is the top-left pixel, D3D's convention.

```glsl
#version 300 es
layout(location = 0) in vec2 position;
layout(location = 1) in vec2 texcoord;
out vec2 uv;
void main() {
    gl_Position = vec4(position.x * 2.0 - 1.0, position.y * -2.0 + 1.0, 1.0, 1.0);
    uv = texcoord;
}
```

### Clear pixel shader (blob 0)

```
ps_4_0
dcl_output o0.xyzw
mov o0.xyzw, l(0, 0, 0, 0)
ret
```

No resources. Used for the strips outside the box and for clearing B on hide.

```glsl
#version 300 es
precision highp float;
out vec4 frag;
void main() { frag = vec4(0.0); }
```

### Trail pixel shader (blob 4)

Binds `cb0` (`gaze` to `trail` used), `s0`, `t0` (2D float, the previous
accumulation). Disassembly:

```
ps_4_0
dcl_constantBuffer cb0[9], immediateIndexed
dcl_sampler s0
dcl_resource_texture2d (float,float,float,float) t0
dcl_input_ps linear v1.xy
dcl_output o0.xyzw
dcl_temps 2
mov r0.x, cb0[3].x
mov r0.y, l(1.00000000e+00)
mul r0.zw, r0.xxxy, v1.xxxy
mad r1.xy, cb0[0].xyxx, r0.xyxx, -r0.zwzz
dp2 r1.x, r1.xyxx, r1.xyxx
sqrt r1.x, r1.x
div_sat r1.x, r1.x, cb0[6].x
add r1.x, -r1.x, l(1.00000000e+00)
mad r1.yz, cb0[1].xxyx, r0.xxyx, -r0.zzwz
mad r0.zw, cb0[2].xxxy, r0.xxxy, -r0.zzzw
dp2 r0.z, r0.zwzz, r0.zwzz
sqrt r0.z, r0.z
dp2 r0.w, r1.yzyy, r1.yzyy
sqrt r0.w, r0.w
div_sat r0.zw, r0.zzzw, cb0[6].x
add r0.w, -r0.w, r1.x
add r0.w, r0.w, l(1.00000000e+00)
add r0.z, -r0.z, r0.w
add r0.z, r0.z, l(1.00000000e+00)
mul r0.z, r0.z, cb0[5].x
sample r1.xyzw, v1.xyxx, t0.xyzw, s0
mul r1.yz, r0.xxyx, cb0[0].xxyx
mad r0.xy, v1.xyxx, r0.xyxx, -r1.yzyy
dp2 r0.x, r0.xyxx, r0.xyxx
sqrt r0.x, r0.x
add_sat r0.y, r0.x, -cb0[6].x
lt r0.x, r0.x, cb0[3].x
and r0.x, r0.x, l(1065353216)
add r0.w, -cb0[8].x, l(1.00000000e+00)
mul r0.x, r0.x, r0.w
mul r0.x, r0.y, r0.x
mad_sat r0.x, -r0.x, l(5.00000000e+00), cb0[4].x
mad o0.x, r1.x, r0.x, r0.z
mov o0.yzw, l(0, 0, 0, 1.00000000e+00)
ret
```

Math, with s = (aspect, 1) so that distances are in units of the screen
height and circles are round in pixels:

- `di = length(gaze_i * s - uv * s)` for the three points.
- Splat: `((1 - sat(d0 / radius)) - sat(d1 / radius) + 1 - sat(d2 / radius) + 1) * strength`,
  that is three cones of radius `Size * H` pixels and height 1, summed and
  scaled by Strength. Absent points are far away and add 0.
- Decay factor: `sat(decay - 5 * sat(d0 - radius) * (d0 < aspect ? 1 : 0) * (1 - trail))`.
  Away from `gaze` (this frame's first point: the 1/5 point, or the only
  point) the trail fades faster, more so for small Trail; at Trail 1 the factor is
  just Decay. `d0 < aspect` is false only when `gaze` is absent (it is then
  about 100000 / H away), so frames without a new point decay by Decay alone.
- Output red = previous red x decay factor + splat. Only red exists in the
  `R32_FLOAT` target; it is unbounded and not clamped.

With the managed values (Strength 0.95, Decay 0.95) a steady gaze adds 2.85 per
frame at the centre and settles near 2.85 / 0.05 = 57 (inference from the
formula).

```glsl
#version 300 es
precision highp float;
layout(std140) uniform Params {
    vec4 gaze, gaze1, gaze2, aspect, decay, strength, radius, color, trail, heatmap, background_color;
};
uniform sampler2D previous;
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
```

### Composite pixel shader (blob 2)

Binds `cb0` (`color`, `heatmap`, `background_color` used), `s0` and a
two-element texture array `tex` in `t0` (accumulation) and `t1` (gradient).

```
ps_4_0
dcl_constantBuffer cb0[11], immediateIndexed
dcl_sampler s0
dcl_resource_texture2d (float,float,float,float) t0
dcl_resource_texture2d (float,float,float,float) t1
dcl_input_ps linear v1.xy
dcl_output o0.xyzw
dcl_temps 3
sample r0.xyzw, v1.xyxx, t0.xyzw, s0
add r0.y, -cb0[9].x, l(1.00000000e+00)
mul r0.z, r0.x, r0.y
mul r0.z, r0.z, l(1.29999995e-01)
mad_sat r1.x, cb0[9].x, r0.x, r0.z
mov r1.y, l(0)
sample r1.xyzw, r1.xyxx, t1.xyzw, s0
mov_sat r0.x, r1.w
add r0.z, -r0.x, l(1.00000000e+00)
mul r2.xyzw, r0.z, cb0[10].xyzw
mad r2.xyzw, cb0[7].xyzw, r0.x, r2.xyzw
mul r0.xyzw, r0.y, r2.xyzw
mul r1.w, r1.w, cb0[7].w
mad r0.xyzw, cb0[9].x, r1.xyzw, r0.xyzw
mul o0.xyz, r0.w, r0.xyzx
mov o0.w, r0.w
ret
```

`0.13` is `0x3e051eb8` (0.129999995). With heatmap = 0 (always):

- `v` = accumulation sampled bilinearly at the full-resolution pixel (the
  quarter texture is magnified by the linear sampler, clamped at the edges).
- `u = sat(0.13 * v)`, `a = sat(gradient(u, 0).a)`.
- `c = background_color * (1 - a) + color * a`.
- Output premultiplied: `(c.rgb * c.a, c.a)`.

The heatmap branch (mode 2, dead) would output the gradient's own colour with
alpha `gradient.a * color.a`, mixed by `heatmap`.

With the defaults (Silver, transparent background) a pixel is
`(0.753 a^2, 0.753 a^2, 0.753 a^2, a)`, a premultiplied grey of alpha a over
the desktop (DWM composes premultiplied alpha).

```glsl
#version 300 es
precision highp float;
layout(std140) uniform Params {
    vec4 gaze, gaze1, gaze2, aspect, decay, strength, radius, color, trail, heatmap, background_color;
};
uniform sampler2D accumulation;
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
```

## Gradient

`0x180001290` uploads one of three 1536 x 1 `R8G8B8A8_UNORM` tables from
`.data` (row pitch 6144) as an immutable shader resource (static): mode 1
`0x180009040`, mode 0 `0x18000a840`, otherwise `0x18000c040`. Only mode 0's
table is ever used.

Mode 0 (`0x18000a840`, SHA-256 of its 6144 bytes
`62efe8b57e14c4987a682ec60163b4fe5bfee1c3693d6f1d57fff2c1ff54f16e`): every
texel is white (RGB 254 or 255, dither noise). Alpha by texel index i:

- rises about linearly from 0 at i = 0 to 255 at i = 256,
- stays 255 from 256 to 320,
- falls about linearly to 0 at i = 369 (last non-zero texel 367),
- is 0 for the remaining 1167 texels.

The piecewise-linear fit `255 i / 256` for i < 256, 255 up to 320, then
`255 (369 - i) / 49` down to 0 is within 5.8/255 of every texel (mean error
0.28/255). In terms of the accumulation value v (u = 0.13 v, texel = 1536 u):
transparent at v = 0, opaque from v = 1.28 to 1.60, transparent again from
v = 1.85. Because a steady gaze settles near 57, the centre of the bubble is
transparent and the visible shape is a ring around the gaze point, solid where
the accumulation passes 1.28 to 1.60 on its way down to zero at the edge of
the cone. For a perfectly still point the steady value is `57 (1 - d / r)`, so
the visible ring lies between 96.8 % and 100 % of the splat radius, opaque from
97.2 % to 97.8 %; at quarter resolution with bilinear magnification and real
gaze jitter it is a soft ring a few pixels wide. Moving gaze leaves a fading
trail that shows while each texel decays through the same band, about 7 frames
at Decay 0.95 (inference from the formulas; not observed).

Mode 1 (`0x180009040`) is the same rise to 255 at i = 256 and then stays
opaque; mode 2 (`0x18000c040`) is a blue to green to yellow to red heat ramp
with alpha 0 for the first texels. Neither is reachable.

## Notes for the GL port

Inference, for the GTK overlay; none of this is Tobii's code.

- Run the trail pass once per update tick, as Tobii does (one render per
  `GazeNative_Update`, nominally every 12 ms). Decay is per frame, so a
  different frame rate changes how long the trail lasts; if the GL loop is
  tied to the display refresh instead, use `decay^(dt / 0.012)` and scale the
  extra decay term to match, or keep a 12 ms tick.
- Orientation: GL textures and framebuffers have their origin at the bottom
  left. Using Tobii's vertex shader unchanged with render-to-texture flips the
  history vertically every frame. Keep one convention in every pass, for
  example `gl_Position = vec4(2x - 1, 2y - 1, 1, 1)` with `uv = position`,
  and upload `gaze.y` as `(H - y) / H`; GL scissor rectangles also count from
  the bottom, so the box becomes `(L, H - B, R - L, B - T)` (and the quarter
  version of it).
- `R32F` is colour-renderable and linearly filterable on desktop GL. On a
  GLES 3.0 context it needs `EXT_color_buffer_float` and
  `OES_texture_float_linear`; `R16F` (filterable in ES 3.0, renderable with
  `EXT_color_buffer_half_float` or `EXT_color_buffer_float`) has enough range
  and precision for values up to about 60 with the 1.28 to 1.85 band.
- Clear both accumulation textures to 0 at creation and on hide.
- The window output is premultiplied alpha; GTK and the compositor expect
  premultiplied colour, so write the composite output unchanged and clear to
  (0, 0, 0, 0). Disable blending (Tobii's composite blend is ONE, ZERO).
- The overlay must be input-transparent (an empty input region), above other
  windows, not focusable and absent from the taskbar, covering the tracked
  monitor.

## Not recovered

- How the overlay looks and its real tick rate on Windows: it was not run
  here, so there is no live evidence. The ring shape, the trail length and the
  frame rate above follow from the code and the managed defaults only.

Ghidra's pseudocode drops a few call arguments; the instructions supply them:
`RSSetState(0x18000deb0)` at `0x180001a6b` and
`ResizeBuffers(2, W or 1, H or 1, R8G8B8A8_UNORM, 0)` at `0x180002fcb`
(static).

# WEBGL1 (LEDGER SR136) — evidence

`unaos/libs/web/gl_core`: a `no_std` + `alloc`, zero-dependency software OpenGL ES 2.0 / WebGL 1.0
pipeline (state machine, GLSL ES 1.00 compiler to a register bytecode, interpreter, rasterizer, textures,
framebuffers), and `handlers/aether/src/js/webgl.rs`, the `WebGLRenderingContext` binding over it. No
system GL, no ANGLE (R83, R104). Branch `exec-host-webgl1`, cut from `558dbdd0`, seven commits, one per
stage; the first commit body (`0fcc98d6`) carries the whole design (state-machine layout, the register
bytecode, the rasterizer), each later body the stage it implements with clause citations.

Specifications: OpenGL ES 2.0.25 ("ES 2.0 §n"), the OpenGL ES Shading Language 1.00.17 ("GLSL ES §n"),
the WebGL 1.0 specification (cited by section TITLE: this seat had no route to the Khronos registry to
verify section numbers, `getaddrinfo ENOTFOUND registry.khronos.org`, 2026-10-13).

## Status of the measurements

| what | status |
|---|---|
| `cargo check -p gl_core -p aether --tests` | rc=0 at every commit of the series (R107: one compile per commit) |
| `cargo test -p gl_core`, `cargo test -p aether --test webgl` | **not run here** (R107: the seat runs every test once, at the fold) |
| ops / pixel | **from reading** the code generator (below); the measured figure is the `WEBGL1 ops/pixel = …` line `tests/pixels.rs::ops_per_pixel_256` prints under `--nocapture` at the fold |
| stages 4–5 lib build | the `--tests` check compiled gl_core only in test mode (std linked by the harness), so `f64::round/floor/ceil` in raster.rs (std-only) passed it; the no_std lib's own build first ran at stage 6 when aether linked it, where they were replaced by `fmath` (commit body of stage 6) |

## Counts

| what | count | where |
|---|---|---|
| WebGL 1.0 IDL constants installed | 296 | `gl_core::consts::TABLE` (one macro emits each `pub const` and its row) |
| WebGLRenderingContext operations | 136 | `handlers/aether/src/js/webgl.rs` `METHODS` (the stage-6 commit body says 137: a miscount, the table has 136) |
| attributes | 4 | canvas, drawingBufferWidth, drawingBufferHeight, drawingBufferColorSpace |

## Ops per pixel (from reading `glsl/codegen.rs`)

| fragment shader | ops / fragment | the ops |
|---|---|---|
| `gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0);` (flat) | 6 | constructor: 4 masked Movs (one per component); store: 1 Mov; Ret |
| `gl_FragColor = vec4(v, 1.0);` (gradient, varying vec3) | 6 | constructor: 3 + 1 Movs; store: 1 Mov; Ret |
| `gl_FragColor = texture2D(t, uv);` (textured) | 3 | Tex; store Mov; Ret |

The gradient vertex shader (`v = col; gl_Position = vec4(pos, 0.0, 1.0);`) is 7 ops per vertex. A shader
that samples a mipmapped texture with implicit LOD (or uses OES_standard_derivatives) runs two helper
invocations per fragment, so it costs 3× its op count; shaders that do not pay nothing.

## Feature table

| feature | spec | state |
|---|---|---|
| error flags, getError one at a time, CONTEXT_LOST_WEBGL once | ES 2.0 §2.5, WebGL "lose the context" | done |
| buffers (ARRAY / ELEMENT_ARRAY, target fixed by first bind, bufferData size / data, bufferSubData range) | ES 2.0 §2.9, WebGL "Buffer Object Binding" | done |
| vertex attributes (BYTE / UBYTE / SHORT / USHORT / FLOAT, normalized, stride ≤ 255, offset alignment, current values, range checking at draw) | ES 2.0 §2.7–§2.8, WebGL | done |
| drawArrays / drawElements, all seven modes; UNSIGNED_BYTE / SHORT indices | ES 2.0 §2.6, §2.8 | done |
| OES_element_index_uint (UNSIGNED_INT indices) | extension | done |
| clipping (six planes), viewport, depth range | ES 2.0 §2.12–§2.13 | done |
| triangles: 8 sub-pixel bits, top-left rule, culling, front face, perspective-correct varyings, polygon offset | ES 2.0 §3.5 | done |
| lines (width 1, diamond-exit by major-axis stepping) | ES 2.0 §3.4 | done (width > 1 is clamped: ALIASED_LINE_WIDTH_RANGE = [1, 1]) |
| points (gl_PointSize 1..64, gl_PointCoord upper-left) | ES 2.0 §3.3 | done |
| scissor, stencil (both faces, 8 ops, masks), depth test (24-bit fixed, 16 for DEPTH_COMPONENT16) | ES 2.0 §4.1 | done |
| blending: every factor (incl. SRC_ALPHA_SATURATE, constant colour / alpha) and FUNC_ADD / SUBTRACT / REVERSE_SUBTRACT | ES 2.0 §4.1.6, WebGL "Blending With Constant Color" | done |
| colour / depth / stencil masks, clear | ES 2.0 §4.2 | done |
| dithering | ES 2.0 §4.1.9 | off (permitted no-op); DITHER is queryable |
| multisampling, SAMPLE_COVERAGE, SAMPLE_ALPHA_TO_COVERAGE | ES 2.0 §3.2 | refused: `antialias` is granted false, SAMPLES = 0; the coverage state is stored and queryable only |
| readPixels RGBA / UNSIGNED_BYTE, PACK_ALIGNMENT, outside pixels untouched | ES 2.0 §4.3.1, WebGL "Reading Pixels Outside the Framebuffer" | done (other format/type pairs INVALID_OPERATION: the implementation read pair is RGBA / UNSIGNED_BYTE) |
| textures 2D and cube; ALPHA / LUMINANCE / LUMINANCE_ALPHA / RGB / RGBA at UNSIGNED_BYTE; 565 / 4444 / 5551 | ES 2.0 §3.7.1 | done |
| texSubImage2D, copyTexImage2D, copyTexSubImage2D | ES 2.0 §3.7.2 | done |
| UNPACK_ALIGNMENT, UNPACK_FLIP_Y_WEBGL, UNPACK_PREMULTIPLY_ALPHA_WEBGL | ES 2.0 §3.6, WebGL "Pixel Storage Parameters" | done; UNPACK_COLORSPACE_CONVERSION_WEBGL stored, conversion is identity |
| generateMipmap (box filter, POT only) | ES 2.0 §3.7.11 | done |
| NEAREST / LINEAR, the four mipmap min filters, REPEAT / CLAMP_TO_EDGE / MIRRORED_REPEAT, NPOT rule, incomplete = (0,0,0,1) | ES 2.0 §3.7.7–§3.7.10, §3.8.2 | done |
| implicit-LOD mip selection | ES 2.0 §3.7.7 | done by helper invocations (the right and upper neighbours' coordinates), not per-primitive estimates |
| compressed textures | ES 2.0 §3.7.3 | refused (COMPRESSED_TEXTURE_FORMATS empty; compressedTex* INVALID_ENUM) |
| OES_texture_float, OES_texture_half_float | extensions | refused (FLOAT / HALF_FLOAT types INVALID_ENUM; storage is RGBA8) |
| renderbuffers RGBA4 / RGB565 / RGB5_A1 / DEPTH_COMPONENT16 / STENCIL_INDEX8 / DEPTH_STENCIL | ES 2.0 §4.4.2, WebGL "Framebuffer Object Attachments" | done |
| framebuffers, completeness (attachment, missing, dimensions, unsupported combos), render to texture | ES 2.0 §4.4 | done |
| feedback loops (sampling the attached texture; copying from it into it) | WebGL "Feedback Loops Between Textures and the Framebuffer" | INVALID_OPERATION |
| GLSL preprocessor (#define with arguments, #if/#elif/#ifdef, defined, #extension, #version 100, #line, #error, #pragma) | GLSL ES §3.4 | done |
| GLSL parser (the §9 grammar), reserved words and operators | GLSL ES §3.6, §5.1, §9 | done |
| GLSL semantics: types without implicit conversion, overloads, qualifiers, precision (FS default), constructors, l-values, structs, arrays, recursion refused | GLSL ES §4–§6 | done |
| Appendix A loop form (for only; while / do-while refused) | GLSL ES Appendix A §4, WebGL "Supported GLSL Constructs" | enforced |
| Appendix A indexing restrictions (constant-index-expressions for samplers / FS uniforms / varyings / temporaries) | GLSL ES Appendix A §5 | lenient: every form of indexing is accepted and executed (index clamped) |
| every builtin function of §8 (trig, exponential, common, geometric, matrix, vector relational, texture incl. Proj / Lod / bias) | GLSL ES §8 | done |
| OES_standard_derivatives (dFdx / dFdy / fwidth, the hint) | extension | done (helper invocations) |
| builtin variables and constants (gl_Position, gl_PointSize, gl_FragCoord, gl_FrontFacing, gl_PointCoord, gl_FragColor, gl_FragData[1], gl_DepthRange, gl_Max*) | GLSL ES §7 | done |
| invariant | GLSL ES §4.6 | parsed, checked across the link (all evaluation is invariant: one interpreter) |
| link: varyings, uniforms across stages, attribute locations, budgets, getUniformLocation names, active info | ES 2.0 §2.10 | done |
| precision formats | WebGL getShaderPrecisionFormat | every precision reports highp (f32 lanes) |
| getParameter: every pname of the WebGL 1.0 table | WebGL getParameter | done |
| WEBGL_lose_context, webglcontextlost / webglcontextrestored (tasks), restore after preventDefault | WebGL "lose the context", "restore the context" | done |
| getContext('webgl' / 'experimental-webgl') one [SameObject] per canvas, null across 2D / WebGL | HTML §4.12.5 | done |
| getContextAttributes (alpha, depth, stencil, premultipliedAlpha, preserveDrawingBuffer, powerPreference, desynchronized, failIfMajorPerformanceCaveat) | WebGL "WebGLContextAttributes" | done; antialias granted false |
| compositing: premultipliedAlpha, alpha:false opaque, preserveDrawingBuffer:false clear-after-present | WebGL "The Drawing Buffer", "Premultiplied Alpha, Canvas APIs and texImage2D" | done |
| toDataURL / drawImage of a GL canvas; texImage2D from ImageData, img, canvas (2D or GL), video, ImageBitmap | WebGL texImage2D, HTML canvas | done |
| OffscreenCanvas getContext('webgl'), webgl2, transferControlToOffscreen | — | refused (outside this arc's files: js/canvas.rs's OffscreenCanvas getContext is unchanged) |
| other extensions (ANGLE_instanced_arrays, OES_vertex_array_object, WEBGL_depth_texture, EXT_texture_filter_anisotropic, WEBGL_debug_renderer_info, …) | — | refused by name: getExtension → null, ledgered `webgl-extension-refused:<name>` |
| origin-clean / cross-origin image checks for texImage2D | WebGL "Origin Restrictions" | not enforced (the 2D canvas in this tree tracks no taint either) |
| makeXRCompatible | — | not present |

## Tests (compiled, run at the fold)

`unaos/libs/web/gl_core/tests/pixels.rs`

| test | asserts |
|---|---|
| `flat_triangle` | a red triangle on a 64×64 buffer: inside / outside / either side of the hypotenuse; a two-triangle quad drawn ONE/ONE at 0.5 is 128 on every pixel (top-left rule: no pixel twice, none missed) |
| `gradient_triangle` | per-vertex RGB interpolated: three pixels equal round(255·bᵢ) of their centre's barycentrics ±1 |
| `textured_quad_nearest_vs_linear` | 2×2 texture on a 4×4 quad: NEAREST gives the quadrant colours; LINEAR gives (191, 64, 0) at pixel (1, 0) (3:1 red/green), clamped corners exact |
| `depth_ordering` | LESS keeps the near green quad over far red drawn before and after it; GREATER then lets red through |
| `alpha_blending` | SRC_ALPHA / ONE_MINUS_SRC_ALPHA of (1,0,0,0.5) over opaque blue = (128, 0, 128, 191) ±1 |
| `builtin_categories` | one pixel per category — angle/trig, exponential, common, geometric, matrix, vector relational, functions/loops/arrays/structs (in/inout/out, dynamic indexing, int division), texture lookup (texture2D, texture2DProj, bias) — each green only when every value matches to 1e-3 |
| `derivatives_and_mip_selection` | dFdx / dFdy / fwidth of gl_FragCoord = 1 (helper invocations ran); a 4×4 checker drawn at 1×1 with NEAREST_MIPMAP_NEAREST samples its grey 1×1 level |
| `ops_per_pixel_256` | a 256×256 gradient triangle covers half the buffer; prints `WEBGL1 ops/pixel`; ≤ 8 ops per fragment |
| `points_and_lines` | a 5-pixel point covers its 5×5 square; a horizontal line covers columns 0..14 of its row, the last pixel excluded |

`unaos/libs/web/gl_core/tests/errors.rs`

| test | asserts |
|---|---|
| `error_table` | 63 rows, each one call → its ES 2.0 / WebGL error (INVALID_ENUM / VALUE / OPERATION / FRAMEBUFFER_OPERATION), then NO_ERROR |
| `uniform_rules` | several values into a non-array, a float setter on an int, transpose = true, a stale-link location: the errors; a null location ignored; getUniform round-trips |
| `framebuffer_completeness` | MISSING_ATTACHMENT, a LUMINANCE colour texture INCOMPLETE_ATTACHMENT, mismatched depth DIMENSIONS, depth + depth-stencil UNSUPPORTED, then a complete one clears and reads back |
| `lost_context` | CONTEXT_LOST_WEBGL once, then NO_ERROR; objects do not survive a restore |
| `compile_errors` | 16 shaders GLSL ES 1.00 / WebGL reject (no FS float precision, while, recursion, undeclared, int→float, gl_ / webgl_ names, %, writing a uniform, #version 300 es, texture2DLod in an FS, dFdx without the extension, swizzle range, constant index range, undefined macro in #if, gl_FragColor + gl_FragData) fail with an `ERROR: 0:` log; each corrected twin compiles |
| `preprocessor` | a function-like macro, #ifdef GL_ES / #else, __LINE__ compile |

`handlers/aether/tests/webgl.rs`

| test | asserts |
|---|---|
| `smoke_draws_a_triangle` | the three.js-style sequence (two shaders, link, Float32Array buffer, attribute, uniforms, clear, drawArrays): LINK_STATUS true, getError 0, readPixels red inside / blue outside, ARRAY_BUFFER_BINDING / CURRENT_PROGRAM return the created objects, VIEWPORT, drawingBufferWidth, active attribute / uniform info, VERSION, the supported extensions |
| `get_context_same_object` | getContext('webgl') === itself === 'experimental-webgl'; instanceof; .canvas; a WebGL canvas answers null to '2d' and a 2D canvas null to 'webgl'; the constants on the interface and the instance; getContextAttributes |
| `lose_and_restore_context` | loseContext: lost at once, CONTEXT_LOST_WEBGL once, createBuffer null, no event yet; after the task webglcontextlost (a WebGLContextEvent) fired; preventDefault lets restoreContext queue webglcontextrestored; the restored context creates buffers, getError 0 |
| `surface_hand_off` | toDataURL of the GL canvas ≠ a blank canvas's; drawImage into a 2D canvas puts the red triangle bottom-left and blue top-right; texImage2D from an ImageData reads back through a framebuffer |

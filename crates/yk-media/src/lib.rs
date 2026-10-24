//! libmpv 的最小 FFI 封装（软件渲染路径）。
//!
//! 为什么是**动态加载**而不是链接：libmpv 由 Homebrew 装在 `/opt/homebrew/lib`，
//! 用 `#[link]` 需要额外配置 `-L` + `rpath`，动态加载则一行路径就够，
//! 而且找不到库时能给出明确错误而不是链接期崩。
//!
//! 为什么是**软件渲染**（`MPV_RENDER_API_TYPE_SW`）：GPUI 的 `surface()` 只接受
//! `CVPixelBuffer`。硬件渲染（Metal/OpenGL）产出的纹理不能直接喂给它，
//! 走 SW 一是实现简单，二是拿到的 RGBA 缓冲区可以填进任意后端。
//! 性能上：解码仍是硬解（`hwdec=auto`），只有「上屏前」回读这一次；
//! 真机若帧率不够，再换 Metal 纹理共享（libmpv 的 `render_metal`）。
//!
//! 这是**验证级**封装：只覆盖「打开文件 → 渲染一帧 → 拿到 RGBA」，
//! 不含音频同步、seek、事件分发 —— 那些属于后续。

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;

use libloading::{Library, Symbol};

/// libmpv 的常见位置（Homebrew 优先）。
const CANDIDATES: [&str; 4] = [
    "/opt/homebrew/lib/libmpv.2.dylib",
    "/usr/local/lib/libmpv.2.dylib",
    "/usr/lib/libmpv.2.dylib",
    "libmpv.2.dylib",
];

// ================= FFI 类型 =================

type MpvHandle = c_void;
type MpvRenderCtx = c_void;

#[repr(C)]
struct MpvRenderParam {
    type_: c_int,
    data: *mut c_void,
}

const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
const MPV_RENDER_PARAM_SW_SIZE: c_int = 17;
const MPV_RENDER_PARAM_SW_FORMAT: c_int = 18;
const MPV_RENDER_PARAM_SW_STRIDE: c_int = 19;
const MPV_RENDER_PARAM_SW_POINTER: c_int = 20;
const MPV_RENDER_PARAM_INVALID: c_int = 0;

const MPV_FORMAT_FLAG: c_int = 3;

// ================= 错误 =================

#[derive(Debug)]
pub enum MpvError {
    Library(String),
    Create,
    Init(i32),
    Command(i32),
    RenderCtx(i32),
    Render(i32),
    Timeout,
}

impl std::fmt::Display for MpvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Library(m) => write!(f, "找不到 libmpv：{m}"),
            Self::Create => write!(f, "mpv_create 失败"),
            Self::Init(c) => write!(f, "mpv_initialize 返回 {c}"),
            Self::Command(c) => write!(f, "mpv_command 返回 {c}"),
            Self::RenderCtx(c) => write!(f, "mpv_render_context_create 返回 {c}"),
            Self::Render(c) => write!(f, "mpv_render_context_render 返回 {c}"),
            Self::Timeout => write!(f, "等待文件加载超时"),
        }
    }
}

impl std::error::Error for MpvError {}

// ================= 播放器 =================

/// 一帧 RGBA（其实是 `rgb0`，第 4 字节是垃圾值，填 buffer 时要写 255）。
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4`，格式 rgb0。
    pub data: Vec<u8>,
}

pub struct MpvPlayer {
    _lib: Library,
    ctx: *mut MpvHandle,
    render: *mut MpvRenderCtx,
    width: u32,
    height: u32,
    buf: Vec<u8>,
    stride: usize,
    // 函数指针（库要活到进程结束，所以连同 Library 一起持有）
    render_render: unsafe extern "C" fn(*mut MpvRenderCtx, *mut MpvRenderParam) -> c_int,
    render_free: unsafe extern "C" fn(*mut MpvRenderCtx),
    terminate: unsafe extern "C" fn(*mut MpvHandle),
    wait_event: unsafe extern "C" fn(*mut MpvHandle, f64) -> *mut c_void,
}

unsafe impl Send for MpvPlayer {}

impl MpvPlayer {
    /// 创建一个离屏播放器（无窗口、无音频输出），尺寸用于 SW 渲染目标。
    pub fn new(width: u32, height: u32) -> Result<Self, MpvError> {
        let lib = Self::load_lib()?;

        // 把这个块单独隔离：`Symbol` 借用 `lib`，块结束即释放，
        // 之后才能把 `lib` move 进 Self。
        let (ctx, render, render_fn, free_fn, term_fn, wait_fn) = unsafe {
            let create: unsafe extern "C" fn() -> *mut MpvHandle =
                *lib.get(b"mpv_create").map_err(|e| MpvError::Library(e.to_string()))?;
            let init: unsafe extern "C" fn(*mut MpvHandle) -> c_int =
                *lib.get(b"mpv_initialize").map_err(|e| MpvError::Library(e.to_string()))?;
            let set_opt: unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int =
                *lib.get(b"mpv_set_option_string").map_err(|e| MpvError::Library(e.to_string()))?;
            let render_create: unsafe extern "C" fn(
                *mut *mut MpvRenderCtx,
                *mut MpvHandle,
                *mut MpvRenderParam,
            ) -> c_int = *lib
                .get(b"mpv_render_context_create")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            let render_fn: unsafe extern "C" fn(*mut MpvRenderCtx, *mut MpvRenderParam) -> c_int =
                *lib.get(b"mpv_render_context_render")
                    .map_err(|e| MpvError::Library(e.to_string()))?;
            let free_fn: unsafe extern "C" fn(*mut MpvRenderCtx) =
                *lib.get(b"mpv_render_context_free")
                    .map_err(|e| MpvError::Library(e.to_string()))?;
            let term_fn: unsafe extern "C" fn(*mut MpvHandle) =
                *lib.get(b"mpv_terminate_destroy")
                    .map_err(|e| MpvError::Library(e.to_string()))?;
            let wait_fn: unsafe extern "C" fn(*mut MpvHandle, f64) -> *mut c_void =
                *lib.get(b"mpv_wait_event")
                    .map_err(|e| MpvError::Library(e.to_string()))?;

            let ctx = create();
            if ctx.is_null() {
                return Err(MpvError::Create);
            }

            // 离屏：不要窗口、不要音频设备（音频后续由 AVSampleBufferAudioRenderer 承担）
            for (k, v) in [
                ("vo", "libmpv"), // 用 render API 必须 libmpv，不能 null
                ("ao", "null"),
                ("vid", "1"),
                ("hwdec", "auto"),
                ("keep-open", "yes"),
            ] {
                let ck = CString::new(k).unwrap();
                let cv = CString::new(v).unwrap();
                set_opt(ctx, ck.as_ptr(), cv.as_ptr());
            }

            let rc = init(ctx);
            if rc < 0 {
                term_fn(ctx);
                return Err(MpvError::Init(rc));
            }

            // ---- 建 SW 渲染上下文 ----
            let api = CString::new("sw").unwrap();
            let mut params = [
                MpvRenderParam {
                    type_: MPV_RENDER_PARAM_API_TYPE,
                    data: api.as_ptr() as *mut c_void,
                },
                MpvRenderParam { type_: MPV_RENDER_PARAM_INVALID, data: ptr::null_mut() },
            ];
            let mut render: *mut MpvRenderCtx = ptr::null_mut();
            let rc = render_create(&mut render, ctx, params.as_mut_ptr());
            if rc < 0 || render.is_null() {
                term_fn(ctx);
                return Err(MpvError::RenderCtx(rc));
            }

            (ctx, render, render_fn, free_fn, term_fn, wait_fn)
        };

        let stride = (width * 4) as usize;
        let buf = vec![0u8; stride * height as usize];

        Ok(Self {
            _lib: lib,
            ctx,
            render,
            width,
            height,
            buf,
            stride,
            render_render: render_fn,
            render_free: free_fn,
            terminate: term_fn,
            wait_event: wait_fn,
        })
    }

    fn load_lib() -> Result<Library, MpvError> {
        let mut last = String::new();
        for p in CANDIDATES {
            match unsafe { Library::new(p) } {
                Ok(l) => return Ok(l),
                Err(e) => last = format!("{p}: {e}"),
            }
        }
        Err(MpvError::Library(last))
    }

    /// 打开一个视频文件并等它加载完成。
    pub fn load_file(&mut self, path: &str) -> Result<(), MpvError> {
        unsafe {
            let cmd: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int =
                *self._lib.get(b"mpv_command").map_err(|e| MpvError::Library(e.to_string()))?;
            let c_load = CString::new("loadfile").unwrap();
            let c_path = CString::new(path).unwrap();
            let args = [c_load.as_ptr(), c_path.as_ptr(), ptr::null()];
            let rc = cmd(self.ctx, args.as_ptr());
            if rc < 0 {
                return Err(MpvError::Command(rc));
            }
        }

        // 等 FILE_LOADED（事件 8）；最多 10 秒
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let ev = unsafe { (self.wait_event)(self.ctx, 0.2) };
            if ev.is_null() {
                continue;
            }
            // mpv_event { event_id: i32, error: i32, ... }
            let event_id = unsafe { *(ev as *const i32) };
            if event_id == 8 {
                return Ok(());
            }
            if event_id == 7 {
                // MPV_EVENT_END_FILE
                return Err(MpvError::Command(-1));
            }
        }
        Err(MpvError::Timeout)
    }

    /// 渲染当前帧到内部缓冲区。
    pub fn render_frame(&mut self) -> Result<RgbaFrame, MpvError> {
        // MPV_RENDER_PARAM_SW_SIZE 要的是 `int[2]`，不是两个独立 int ——
        // 传单个 int 时 mpv 会把相邻栈内存当作高度，直接触发 mp_image_set_size 断言。
        let mut size = [self.width as c_int, self.height as c_int];
        let c_format = CString::new("rgb0").unwrap();
        let mut stride = self.stride;

        let mut params = [
            MpvRenderParam { type_: MPV_RENDER_PARAM_SW_SIZE, data: size.as_mut_ptr() as *mut c_void },
            MpvRenderParam { type_: MPV_RENDER_PARAM_SW_FORMAT, data: c_format.as_ptr() as *mut c_void },
            MpvRenderParam { type_: MPV_RENDER_PARAM_SW_STRIDE, data: &mut stride as *mut _ as *mut c_void },
            MpvRenderParam { type_: MPV_RENDER_PARAM_SW_POINTER, data: self.buf.as_mut_ptr() as *mut c_void },
            MpvRenderParam { type_: MPV_RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];

        let rc = unsafe { (self.render_render)(self.render, params.as_mut_ptr()) };
        if rc < 0 {
            return Err(MpvError::Render(rc));
        }

        Ok(RgbaFrame {
            width: self.width,
            height: self.height,
            data: self.buf.clone(),
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) {
        unsafe {
            if !self.render.is_null() {
                (self.render_free)(self.render);
                self.render = ptr::null_mut();
            }
            if !self.ctx.is_null() {
                (self.terminate)(self.ctx);
                self.ctx = ptr::null_mut();
            }
        }
    }
}

/// 把 `rgb0` 缓冲区转成真正的 RGBA（第 4 字节填 255）。
///
/// libmpv 的 `rgb0` 第 4 字节是未初始化垃圾值，直接当 alpha 用会出现随机透明。
pub fn rgb0_to_rgba(frame: &RgbaFrame) -> Vec<u8> {
    let mut out = frame.data.clone();
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    out
}

/// 供调试：把一帧存成 PNG。
pub fn save_png(frame: &RgbaFrame, path: &str) -> std::io::Result<()> {
    // 不引 image crate：手写一个最小 PNG（无压缩，用 zlib stored block）。
    let rgba = rgb0_to_rgba(frame);
    let mut raw = Vec::with_capacity(rgba.len() + frame.height as usize);
    for y in 0..frame.height as usize {
        raw.push(0u8); // filter: none
        let row = &rgba[y * frame.width as usize * 4..(y + 1) * frame.width as usize * 4];
        raw.extend_from_slice(row);
    }
    let png = encode_png(frame.width, frame.height, &raw);
    std::fs::write(path, png)
}

fn encode_png(w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            table[i as usize] = c;
        }
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8bit, RGBA

    // zlib: 用 stored (uncompressed) deflate block，省掉压缩实现
    let mut z = vec![0x78, 0x01];
    let mut pos = 0;
    while pos < raw.len() {
        let chunk_len = (raw.len() - pos).min(65535);
        let last = pos + chunk_len >= raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(chunk_len as u16).to_le_bytes());
        z.extend_from_slice(&(!(chunk_len as u16)).to_le_bytes());
        z.extend_from_slice(&raw[pos..pos + chunk_len]);
        pos += chunk_len;
    }
    let mut adler_a: u32 = 1;
    let mut adler_b: u32 = 0;
    for &b in raw {
        adler_a = (adler_a + b as u32) % 65521;
        adler_b = (adler_b + adler_a) % 65521;
    }
    z.extend_from_slice(&((adler_b << 16) | adler_a).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &z));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

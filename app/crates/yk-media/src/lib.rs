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

pub mod convert;
#[cfg(target_os = "macos")]
pub mod pixel_buffer;

use std::ffi::{c_char, c_int, c_void, CString};
use std::path::PathBuf;
use std::ptr;

use libloading::{Library, Symbol};

/// libmpv 的常见位置（Homebrew 优先）。
const CANDIDATES: [&str; 4] = [
    "/opt/homebrew/lib/libmpv.2.dylib",
    "/usr/local/lib/libmpv.2.dylib",
    "/usr/lib/libmpv.2.dylib",
    "libmpv.2.dylib",
];

/// 加载顺序：环境变量 > .app 内置 > Homebrew 安装位置。
///
/// 打包后的 .app 会把 libmpv 及其依赖放进 `Contents/Frameworks`，
/// 这里按可执行文件位置反推 —— 否则会优先命中开发机上的 Homebrew 副本，
/// 在别人机器上直接播不了视频。
fn candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(explicit) = std::env::var_os("YK_LIBMPV") {
        out.push(PathBuf::from(explicit));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            for name in ["libmpv.2.dylib", "libmpv.dylib"] {
                let path = macos_dir.join("../Frameworks").join(name);
                if path.exists() {
                    out.push(path);
                }
            }
        }
    }
    out.extend(CANDIDATES.iter().map(PathBuf::from));
    out
}

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
    report_swap: unsafe extern "C" fn(*mut MpvRenderCtx),
}

unsafe impl Send for MpvPlayer {}

impl MpvPlayer {
    /// 创建一个离屏播放器（无窗口、无音频输出），尺寸用于 SW 渲染目标。
    pub fn new(width: u32, height: u32) -> Result<Self, MpvError> {
        Self::new_with_start(width, height, 0.0)
    }

    /// 同 [`MpvPlayer::new`]，但从 `start_secs` 开始播放（续播用）。
    ///
    /// 走 mpv 的 `start` 选项：**必须在 `mpv_initialize` 之前设置**，
    /// mpv 会在打开文件时直接定位到该位置 —— 不会先播开头再跳。
    /// `start_secs <= 0` 表示从头播。
    pub fn new_with_start(width: u32, height: u32, start_secs: f64) -> Result<Self, MpvError> {
        let lib = Self::load_lib()?;

        // 把这个块单独隔离：`Symbol` 借用 `lib`，块结束即释放，
        // 之后才能把 `lib` move 进 Self。
        let (ctx, render, render_fn, free_fn, term_fn, wait_fn, report_fn) = unsafe {
            let create: unsafe extern "C" fn() -> *mut MpvHandle = *lib
                .get(b"mpv_create")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            let init: unsafe extern "C" fn(*mut MpvHandle) -> c_int =
                *lib.get(b"mpv_initialize")
                    .map_err(|e| MpvError::Library(e.to_string()))?;
            let set_opt: unsafe extern "C" fn(
                *mut MpvHandle,
                *const c_char,
                *const c_char,
            ) -> c_int = *lib
                .get(b"mpv_set_option_string")
                .map_err(|e| MpvError::Library(e.to_string()))?;
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
            let free_fn: unsafe extern "C" fn(*mut MpvRenderCtx) = *lib
                .get(b"mpv_render_context_free")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            let term_fn: unsafe extern "C" fn(*mut MpvHandle) = *lib
                .get(b"mpv_terminate_destroy")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            let wait_fn: unsafe extern "C" fn(*mut MpvHandle, f64) -> *mut c_void = *lib
                .get(b"mpv_wait_event")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            // SW 渲染后必须 report_swap，否则 mpv 认为「上一帧还没显示」，
            // 不会推进播放（表现就是画面永远停在第一帧）。
            let report_fn: unsafe extern "C" fn(*mut MpvRenderCtx) = *lib
                .get(b"mpv_render_context_report_swap")
                .map_err(|e| MpvError::Library(e.to_string()))?;

            let ctx = create();
            if ctx.is_null() {
                return Err(MpvError::Create);
            }

            // 离屏：不要窗口、不要音频设备（音频后续由 AVSampleBufferAudioRenderer 承担）
            for (k, v) in [
                ("vo", "libmpv"), // 用 render API 必须 libmpv，不能 null
                // 音频交给 mpv 自己播。
                //
                // AO 选 avfoundation 而不是 coreaudio：本机实测 coreaudio
                // 初始化失败（`unable to set the input channel layout`,
                // OSStatus -50），avfoundation 正常。可用 YK_AO 覆盖。
                // libmpv **没有**音频回调 API —— render API 只管视频，
                // 所以不能像 AVFoundation 那样把音频包喂给自定义渲染器。
                // 让 mpv 播音频的好处：音画同步由它的 playloop 保证，
                // 我们只需要按时取视频帧。
                (
                    "ao",
                    std::env::var("YK_AO")
                        .unwrap_or_else(|_| "avfoundation".into())
                        .leak(),
                ),
                // 把 mpv 自己的日志打到 stderr，便于确认 AO 是否初始化成功
                ("terminal", "yes"),
                ("msg-level", "ao=info,vo=info,cplayer=warn"),
                ("vid", "1"),
                ("hwdec", "auto"),
                ("keep-open", "yes"),
                // B 站 CDN 对 Referer/UA 有要求；直接交给 mpv 的 HTTP 层，
                // 避免 m4s 请求 403。
                (
                    "user-agent",
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
                ),
                ("http-header-fields", "Referer: https://www.bilibili.com/"),
            ] {
                let ck = CString::new(k).unwrap();
                let cv = CString::new(v).unwrap();
                set_opt(ctx, ck.as_ptr(), cv.as_ptr());
            }

            // 续播：起播位置用 mpv 的 `start` 选项（必须在 initialize 之前设置）。
            // 选项生效后 mpv 打开文件就直接定位过去，不经过「先播开头再跳」。
            if start_secs.is_finite() && start_secs > 0.0 {
                let key = CString::new("start").unwrap();
                let value = CString::new(format!("{start_secs:.3}")).unwrap();
                set_opt(ctx, key.as_ptr(), value.as_ptr());
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
                MpvRenderParam {
                    type_: MPV_RENDER_PARAM_INVALID,
                    data: ptr::null_mut(),
                },
            ];
            let mut render: *mut MpvRenderCtx = ptr::null_mut();
            let rc = render_create(&mut render, ctx, params.as_mut_ptr());
            if rc < 0 || render.is_null() {
                term_fn(ctx);
                return Err(MpvError::RenderCtx(rc));
            }

            (ctx, render, render_fn, free_fn, term_fn, wait_fn, report_fn)
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
            report_swap: report_fn,
        })
    }

    fn load_lib() -> Result<Library, MpvError> {
        let mut last = String::new();
        for p in candidate_paths() {
            match unsafe { Library::new(&p) } {
                Ok(l) => return Ok(l),
                Err(e) => last = format!("{}: {e}", p.display()),
            }
        }
        Err(MpvError::Library(last))
    }

    /// 打开一个视频文件并等它加载完成。
    pub fn load_file(&mut self, path: &str) -> Result<(), MpvError> {
        self.load_file_with_audio(path, None)
    }

    /// 打开视频；DASH 场景把独立音频作为附加音轨。
    ///
    /// **不要用 `loadfile ... audio-file=...`**：`loadfile` 的第三个参数只接受
    /// 逗号分隔的 per-file 选项，`audio-file` 不是其中之一，mpv 会把它当成
    /// 整数选项解析并报 `The loadfile option must be an integer`。
    /// 正确路径是先 loadfile 视频，再用 `audio-add <url>` 添加音轨。
    pub fn load_file_with_audio(
        &mut self,
        path: &str,
        audio: Option<&str>,
    ) -> Result<(), MpvError> {
        self.load_plain(path)?;
        let Some(audio) = audio else {
            return Ok(());
        };
        unsafe {
            let cmd: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int = *self
                ._lib
                .get(b"mpv_command")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            let c_add = CString::new("audio-add").map_err(|_| MpvError::Command(-1))?;
            let c_audio = CString::new(audio).map_err(|_| MpvError::Command(-1))?;
            let args = [c_add.as_ptr(), c_audio.as_ptr(), ptr::null()];
            let rc = cmd(self.ctx, args.as_ptr());
            if rc < 0 {
                // 音轨失败不应让视频也打不开：保留无声播放，至少画面可用。
                eprintln!("[mpv] audio-add 失败（rc={rc}），视频继续无声播放");
            }
        }
        Ok(())
    }

    fn load_plain(&mut self, path: &str) -> Result<(), MpvError> {
        unsafe {
            let cmd: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int = *self
                ._lib
                .get(b"mpv_command")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            let c_load = CString::new("loadfile").unwrap();
            let c_path = CString::new(path).unwrap();
            let args = [c_load.as_ptr(), c_path.as_ptr(), ptr::null()];
            let rc = cmd(self.ctx, args.as_ptr());
            if rc < 0 {
                return Err(MpvError::Command(rc));
            }
        }
        self.wait_file_loaded()
    }

    fn wait_file_loaded(&mut self) -> Result<(), MpvError> {
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

    /// 暂停 / 继续播放（卡片视频控制栏用）。
    pub fn set_pause(&mut self, paused: bool) -> Result<(), MpvError> {
        unsafe {
            let cmd: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int = *self
                ._lib
                .get(b"mpv_command")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            let c_cmd = CString::new("set").unwrap();
            let c_prop = CString::new("pause").unwrap();
            let c_val = CString::new(if paused { "yes" } else { "no" }).unwrap();
            let args = [c_cmd.as_ptr(), c_prop.as_ptr(), c_val.as_ptr(), ptr::null()];
            let rc = cmd(self.ctx, args.as_ptr());
            if rc < 0 {
                return Err(MpvError::Command(rc));
            }
        }
        Ok(())
    }

    /// 跳到指定位置（秒，绝对定位）。
    ///
    /// 主要给「起播位置没生效时补一刀」用；进度条拖动接上之后也会走这里。
    pub fn seek(&mut self, position_secs: f64) -> Result<(), MpvError> {
        if !position_secs.is_finite() || position_secs < 0.0 {
            return Err(MpvError::Command(-1));
        }
        unsafe {
            let cmd: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int = *self
                ._lib
                .get(b"mpv_command")
                .map_err(|e| MpvError::Library(e.to_string()))?;
            let c_cmd = CString::new("seek").unwrap();
            let c_pos = CString::new(format!("{position_secs:.3}")).unwrap();
            let c_mode = CString::new("absolute").unwrap();
            let args = [
                c_cmd.as_ptr(),
                c_pos.as_ptr(),
                c_mode.as_ptr(),
                ptr::null(),
            ];
            let rc = cmd(self.ctx, args.as_ptr());
            if rc < 0 {
                return Err(MpvError::Command(rc));
            }
        }
        Ok(())
    }

    /// 当前播放位置（秒）；读不到时返回 0。
    pub fn position_secs(&self) -> f64 {
        self.get_property_double("time-pos").unwrap_or(0.0)
    }

    /// 渲染当前帧到内部缓冲区。
    /// 读一个 double 属性（诊断用）。`MPV_FORMAT_DOUBLE = 5`。
    pub fn get_property_double(&self, name: &str) -> Option<f64> {
        unsafe {
            let get: libloading::Symbol<
                unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int,
            > = self._lib.get(b"mpv_get_property").ok()?;
            let c_name = CString::new(name).ok()?;
            let mut out: f64 = 0.0;
            let rc = get(
                self.ctx,
                c_name.as_ptr(),
                5, // MPV_FORMAT_DOUBLE
                &mut out as *mut f64 as *mut c_void,
            );
            if rc < 0 {
                None
            } else {
                Some(out)
            }
        }
    }

    /// 排空 mpv 的事件队列。
    ///
    /// **必须定期调用**：mpv 的客户端 API 是「拉模式」，不取事件它会认为
    /// 调用方没在处理，内部队列满了就不推进解码 —— 表现就是画面停在第一帧。
    pub fn drain_events(&self) {
        loop {
            let ev = unsafe { (self.wait_event)(self.ctx, 0.0) };
            if ev.is_null() {
                break;
            }
            // event_id == 0 表示 MPV_EVENT_NONE：队列已空
            if unsafe { *(ev as *const i32) } == 0 {
                break;
            }
        }
    }

    pub fn render_frame(&mut self) -> Result<RgbaFrame, MpvError> {
        // MPV_RENDER_PARAM_SW_SIZE 要的是 `int[2]`，不是两个独立 int ——
        // 传单个 int 时 mpv 会把相邻栈内存当作高度，直接触发 mp_image_set_size 断言。
        let mut size = [self.width as c_int, self.height as c_int];
        let c_format = CString::new("rgb0").unwrap();
        let mut stride = self.stride;

        let mut params = [
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr() as *mut c_void,
            },
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_SW_FORMAT,
                data: c_format.as_ptr() as *mut c_void,
            },
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_SW_STRIDE,
                data: &mut stride as *mut _ as *mut c_void,
            },
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_SW_POINTER,
                data: self.buf.as_mut_ptr() as *mut c_void,
            },
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let rc = unsafe { (self.render_render)(self.render, params.as_mut_ptr()) };
        if rc < 0 {
            return Err(MpvError::Render(rc));
        }
        unsafe { (self.report_swap)(self.render) };

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
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
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

//! Windows 资源：exe 图标 + 版本信息。
//!
//! 只在 Windows 上生效（交叉编译到别的平台时整段跳过），
//! 需要 Windows SDK 的 `rc.exe`；缺失时直接失败并提示，不要悄悄出一个无图标 exe。

fn main() {
    #[cfg(target_os = "windows")]
    {
        let manifest_dir = std::path::PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR 未设置"),
        );
        let icon = manifest_dir.join("../../../packaging/windows/ykpili.ico");
        println!("cargo:rerun-if-changed={}", icon.display());

        let mut res = winresource::WindowsResource::new();
        res.set_icon(icon.to_str().expect("图标路径不是合法 UTF-8"));
        res.set("ProductName", "YKPili");
        res.set("FileDescription", "YKPili - 哔哩哔哩第三方客户端");
        res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        res.set("FileVersion", env!("CARGO_PKG_VERSION"));
        res.set("CompanyName", "YKPili");
        res.set("LegalCopyright", "GPL-2.0-or-later（含 GPL 版 libmpv）");
        res.compile()
            .expect("编译 Windows 资源失败：确认已安装 Windows SDK 的 rc.exe");
    }
}

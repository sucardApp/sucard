// 编译 Windows 版时，把 assets/icon.ico 嵌进 sucard.exe（资源管理器、任务栏、快捷方式里看到的图标）。
// 其他系统什么都不做。
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // 缺少资源编译器（MSVC 的 rc.exe / GNU 的 windres）时不让整个编译失败，只是没有文件图标
            println!("cargo:warning=没能嵌入 exe 图标：{e}");
        }
    }
}

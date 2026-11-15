// Embed Alibaba PuHuiTi when the font file is present in assets/.
// The font is free for commercial use (阿里巴巴普惠体 3.0, 全社会免费商用)
// but is a binary asset, so it is not pushed to the git remote; builds
// without it fall back to system CJK fonts at runtime.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(puhuiti)");
    let path = std::path::Path::new("assets/AlibabaPuHuiTi-3-55-Regular.ttf");
    println!("cargo:rerun-if-changed=assets/AlibabaPuHuiTi-3-55-Regular.ttf");
    if path.exists() {
        println!("cargo:rustc-cfg=puhuiti");
    }
}

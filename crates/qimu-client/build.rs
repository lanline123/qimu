fn main() {
    embed_exe_icon();
}

// ---------------------------------------------------------------------------
// Windows executable icon (brand logo). The single source of truth is
// assets/logo.png.b64 — the same asset the GUI renders — so the exe,
// the window and the installer can never drift apart.
//
// Best effort by design: any failure (non-Windows target, missing
// asset, decode problem, missing resource compiler) only prints a
// cargo warning; the build never fails because of the icon.
// ---------------------------------------------------------------------------

fn b64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let mut acc: u32 = 0;
        let mut nbits = 0;
        let mut pad = 0;
        for &c in chunk {
            if c == b'=' {
                pad += 1;
                acc <<= 6;
                nbits += 6;
            } else {
                acc = (acc << 6) | val(c)?;
                nbits += 6;
            }
        }
        let real = nbits - 6 * pad;
        if real >= 8 {
            out.push((acc >> (nbits - 8)) as u8);
        }
        if real >= 16 {
            out.push((acc >> (nbits - 16)) as u8);
        }
        if real >= 24 {
            out.push((acc >> (nbits - 24)) as u8);
        }
    }
    Some(out)
}

/// Wrap a PNG in a single-image .ico container (Vista+ PNG-compressed
/// entry — Windows accepts it for exe resources and NSIS alike).
fn png_to_ico(png: &[u8]) -> Option<Vec<u8>> {
    // PNG signature + IHDR width/height (big-endian u32 at 16 / 20).
    if png.len() < 24 || &png[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    if w == 0 || w > 256 || h == 0 || h > 256 {
        return None;
    }
    let mut ico = Vec::with_capacity(22 + png.len());
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&1u16.to_le_bytes()); // image count
    ico.push(if w == 256 { 0 } else { w as u8 });
    ico.push(if h == 256 { 0 } else { h as u8 });
    ico.push(0); // palette colors
    ico.push(0); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // color planes
    ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes()); // offset of image data
    ico.extend_from_slice(png);
    Some(ico)
}

fn embed_exe_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()),
    );
    let b64_path = manifest.join("assets").join("logo.png.b64");
    println!("cargo:rerun-if-changed={}", b64_path.display());
    let Ok(b64) = std::fs::read_to_string(&b64_path) else {
        println!("cargo:warning=qimu-client: assets/logo.png.b64 unreadable; exe icon skipped");
        return;
    };
    let (Some(png),) = (b64_decode(&b64),) else {
        println!("cargo:warning=qimu-client: logo.png.b64 decode failed; exe icon skipped");
        return;
    };
    let Some(ico) = png_to_ico(&png) else {
        println!("cargo:warning=qimu-client: logo PNG not wrappable as .ico; exe icon skipped");
        return;
    };
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap_or_else(|_| ".".into()));
    let ico_path = out_dir.join("qimu.ico");
    if std::fs::write(&ico_path, &ico).is_err() {
        println!("cargo:warning=qimu-client: cannot write qimu.ico; exe icon skipped");
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_string_lossy().as_ref());
    if let Err(e) = res.compile() {
        println!("cargo:warning=qimu-client: icon resource compile failed ({e}); exe icon skipped");
    }
}

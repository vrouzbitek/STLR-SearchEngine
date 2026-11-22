use base64::Engine;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use windows::core::PCWSTR;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

const SIZE_PX: i32 = 48;

// key: full path for apps/exe/lnk (unique icons), ".ext" for plain files, "<dir>" for folders.
// shortcut: generic icon for all folders/plain files, upgrade to per-path if special folder icons matter.
static CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);

pub fn icon(path: &str, kind: &str) -> Option<String> {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let per_path = kind == "app" || matches!(ext.as_str(), "exe" | "lnk" | "ico" | "url" | "appref-ms");
    let key = match (kind, per_path) {
        ("folder", _) => "<dir>".to_string(),
        (_, true) => path.to_lowercase(),
        _ => format!(".{ext}"),
    };
    if let Some(hit) = CACHE.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let png = unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); // per thread; repeat calls are harmless
        if per_path {
            msix_logo_png(path).or_else(|| shell_item_png(path)).or_else(|| type_icon_png(path, false, false))
        } else {
            type_icon_png(path, kind == "folder", true)
        }
    };
    let data = png.map(|b| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)));
    CACHE.lock().unwrap().insert(key, data.clone());
    data
}

/// Shortcuts to packaged (MSIX/Store) apps: the shell renders some of them as a gray plated tile
/// (e.g. Claude), so read the package's own Square44x44Logo PNG instead.
fn msix_logo_png(path: &str) -> Option<Vec<u8>> {
    if !path.to_ascii_lowercase().ends_with(".lnk") {
        return None;
    }
    let app_id = crate::switcher::lnk_app_id(path)?;
    let dir = package_dir(app_id.split('!').next()?)?;
    let manifest = std::fs::read_to_string(dir.join("AppxManifest.xml")).ok()?;
    let rel = manifest_attr(&manifest, "Square44x44Logo")?;
    let base = dir.join(rel.replace('/', "\\"));
    let stem = base.with_extension("");
    let s = stem.to_string_lossy();
    [base.clone(), format!("{s}.scale-200.png").into(), format!("{s}.scale-100.png").into(), format!("{s}.targetsize-48.png").into()]
        .iter()
        .find_map(|p| std::fs::read(p).ok())
        .filter(|b| b.starts_with(b"\x89PNG"))
}

/// First `name="value"` attribute in the manifest (good enough for the logo path; no XML dependency).
fn manifest_attr(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("{name}=\""))? + name.len() + 2;
    Some(xml[start..].split('"').next()?.to_string())
}

/// Install directory of the newest installed package of a family (works for WindowsApps on any drive).
fn package_dir(family: &str) -> Option<std::path::PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::Storage::Packaging::Appx::{GetPackagePathByFullName, GetPackagesByPackageFamily};
    let fam: Vec<u16> = family.encode_utf16().chain([0]).collect();
    unsafe {
        let (mut count, mut len) = (0u32, 0u32);
        let _ = GetPackagesByPackageFamily(PCWSTR(fam.as_ptr()), &mut count, None, &mut len, None);
        if count == 0 || len == 0 {
            return None;
        }
        let mut names = vec![PWSTR::null(); count as usize];
        let mut buf = vec![0u16; len as usize];
        GetPackagesByPackageFamily(PCWSTR(fam.as_ptr()), &mut count, Some(names.as_mut_ptr()), &mut len, Some(PWSTR(buf.as_mut_ptr())))
            .ok()
            .ok()?;
        let full = PCWSTR(names.last()?.0);
        let mut plen = 0u32;
        let _ = GetPackagePathByFullName(full, &mut plen, None);
        let mut pbuf = vec![0u16; plen as usize];
        GetPackagePathByFullName(full, &mut plen, Some(PWSTR(pbuf.as_mut_ptr()))).ok().ok()?;
        let end = pbuf.iter().position(|&c| c == 0).unwrap_or(pbuf.len());
        Some(String::from_utf16_lossy(&pbuf[..end]).into())
    }
}

/// Real icon of this exact file (resolves .lnk targets, Store-app shortcuts, embedded exe icons).
unsafe fn shell_item_png(path: &str) -> Option<Vec<u8>> {
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
        let hbm = factory
            .GetImage(SIZE { cx: SIZE_PX, cy: SIZE_PX }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)
            .ok()?;
        let png = bitmap_png(hbm, None, true);
        let _ = DeleteObject(hbm.into());
        png
    }
}

/// Generic type icon (folder / ".pdf"); `by_attrs` = don't touch the disk.
unsafe fn type_icon_png(path: &str, is_dir: bool, by_attrs: bool) -> Option<Vec<u8>> {
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let mut info = SHFILEINFOW::default();
        let mut flags = SHGFI_ICON | SHGFI_LARGEICON;
        let mut attrs = Default::default();
        if by_attrs {
            flags |= SHGFI_USEFILEATTRIBUTES;
            attrs = if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
        }
        let r = SHGetFileInfoW(PCWSTR(wide.as_ptr()), attrs, Some(&mut info), size_of::<SHFILEINFOW>() as u32, flags);
        if r == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let mut ii = ICONINFO::default();
        let png = if GetIconInfo(info.hIcon, &mut ii).is_ok() {
            let p = if ii.hbmColor.is_invalid() { None } else { bitmap_png(ii.hbmColor, Some(ii.hbmMask), false) };
            let _ = DeleteObject(ii.hbmColor.into());
            let _ = DeleteObject(ii.hbmMask.into());
            p
        } else {
            None
        };
        let _ = DestroyIcon(info.hIcon);
        png
    }
}

unsafe fn bitmap_bgra(dc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
    unsafe {
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        (lines != 0).then_some(px)
    }
}

/// 32-bit HBITMAP -> straight-alpha RGBA PNG. `mask`: AND mask for alpha-less legacy icons.
/// `maybe_premultiplied`: shell image factory hands back premultiplied BGRA.
unsafe fn bitmap_png(hbm: HBITMAP, mask: Option<HBITMAP>, maybe_premultiplied: bool) -> Option<Vec<u8>> {
    unsafe {
        let mut bm = BITMAP::default();
        GetObjectW(hbm.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 {
            return None;
        }
        let dc = GetDC(None);
        let mut px = bitmap_bgra(dc, hbm, w, h);
        if let (Some(p), Some(m)) = (px.as_mut(), mask) {
            if p.chunks(4).all(|c| c[3] == 0) {
                if let Some(mask) = bitmap_bgra(dc, m, w, h) {
                    for (c, m) in p.chunks_mut(4).zip(mask.chunks(4)) {
                        c[3] = 255 - m[0]; // white in AND mask = transparent
                    }
                }
            }
        }
        ReleaseDC(None, dc);
        let mut px = px?;
        prepare_rgba(&mut px, maybe_premultiplied);
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().ok()?.write_image_data(&px).ok()?;
        Some(out)
    }
}

/// BGRA -> RGBA; un-premultiply only if every pixel is consistent with premultiplied data.
fn prepare_rgba(px: &mut [u8], maybe_premultiplied: bool) {
    let premul = maybe_premultiplied && px.chunks(4).all(|c| c[0] <= c[3] && c[1] <= c[3] && c[2] <= c[3]);
    for c in px.chunks_mut(4) {
        c.swap(0, 2);
        if premul && c[3] > 0 && c[3] < 255 {
            for i in 0..3 {
                c[i] = ((c[i] as u32 * 255 + c[3] as u32 / 2) / c[3] as u32) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::prepare_rgba;

    // Dumps old (SHGetFileInfo) vs new (IShellItemImageFactory) icons for real Start Menu apps:
    // STLR_ICON_DUMP=<dir> cargo test --lib dump_icons -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_icons() {
        let dir = std::path::PathBuf::from(std::env::var("STLR_ICON_DUMP").expect("STLR_ICON_DUMP"));
        crate::apps::index();
        for q in ["spotify", "claude", "discord"] {
            let Some(hit) = crate::apps::search(q, 1).pop() else { continue };
            unsafe {
                let _ = super::CoInitializeEx(None, super::COINIT_APARTMENTTHREADED);
                let old = super::type_icon_png(&hit.path, false, false);
                let new = super::msix_logo_png(&hit.path).or_else(|| super::shell_item_png(&hit.path));
                println!("{q}: {} | old {:?} B | new {:?} B", hit.path, old.as_ref().map(|v| v.len()), new.as_ref().map(|v| v.len()));
                let id = crate::switcher::lnk_app_id(&hit.path);
                println!("  app_id {id:?} dir {:?}", id.as_deref().and_then(|i| super::package_dir(i.split('!').next()?)));
                if let Some(b) = old { std::fs::write(dir.join(format!("{q}-old.png")), b).unwrap(); }
                if let Some(b) = new { std::fs::write(dir.join(format!("{q}-new.png")), b).unwrap(); }
            }
        }
    }

    #[test]
    fn unpremultiplies_and_swaps() {
        let mut px = [0, 0, 64, 128, /* opaque */ 10, 20, 30, 255];
        prepare_rgba(&mut px, true);
        assert_eq!(px, [128, 0, 0, 128, 30, 20, 10, 255]);
        let mut straight = [200, 0, 0, 100]; // channel > alpha => not premultiplied, leave as is
        prepare_rgba(&mut straight, true);
        assert_eq!(straight, [0, 0, 200, 100]);
    }
}

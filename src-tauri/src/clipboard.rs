use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

const CF_UNICODETEXT: u32 = 13;

pub fn copy_text(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe {
        OpenClipboard(None).map_err(|e| e.to_string())?;
        let r = (|| -> windows::core::Result<()> {
            EmptyClipboard()?;
            let h = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
            let p = GlobalLock(h) as *mut u16;
            if p.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(h);
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(h.0)))?; // clipboard owns the memory now
            Ok(())
        })();
        let _ = CloseClipboard();
        r.map_err(|e| e.to_string())
    }
}

//! Match Windows 11's native caption to the webview without replacing OS controls.
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
};

fn color(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
pub fn apply(window: &tauri::WebviewWindow, dark: bool) {
    let Ok(hwnd) = window.hwnd() else { return };
    let palette = if dark {
        [color(10, 12, 16), color(232, 234, 240), color(31, 36, 44)]
    } else {
        [
            color(246, 247, 249),
            color(26, 29, 35),
            color(216, 220, 228),
        ]
    };
    for (attribute, value) in [DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_BORDER_COLOR]
        .into_iter()
        .zip(palette)
    {
        // Older Windows versions can reject these optional appearance attributes.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd.0 as _,
                attribute as u32,
                (&value as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn colorref_uses_windows_bgr_byte_order() {
        assert_eq!(super::color(10, 12, 16), 0x00100c0a);
    }
}

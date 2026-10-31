//! Load the profiled, multi-resolution icon directly into the running application.
//! IconServices may retain a differently colour-matched/resampled bundle icon even
//! when the ICNS resource is correct. NSImage decodes the embedded ICC profiles;
//! keeping the full ICNS also avoids upscaling a single small RGBA window icon.
//! The Dock-only asset includes transparent safe area: direct NSImage loading
//! does not apply IconServices' automatic tile inset to the ordinary bundle icon.

use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

pub fn install() -> Result<(), &'static str> {
    let mtm = MainThreadMarker::new().ok_or("Dock icon must be installed on the main thread")?;
    let data = NSData::with_bytes(include_bytes!("../icons/dock-icon.icns"));
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or("Could not decode the bundled Dock icon")?;
    // SAFETY: the application and image are accessed on the main thread; AppKit
    // retains the supplied image. This changes no bundle files or system caches.
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    Ok(())
}

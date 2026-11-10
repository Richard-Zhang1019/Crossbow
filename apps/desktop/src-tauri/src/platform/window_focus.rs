//! Present the main window only in response to an explicit user action.
//! Background notification windows must never use this activation path.

use tauri::WebviewWindow;

pub fn present(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let main = window.clone();
        window.run_on_main_thread(move || {
            // Resolve the native pointer only on AppKit's main thread, after
            // any Accessory -> Regular transition and WebView reconstruction.
            if let (Some(mtm), Ok(pointer)) = (objc2::MainThreadMarker::new(), main.ns_window()) {
                use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWindow};
                let app = NSApplication::sharedApplication(mtm);
                app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
                app.unhideWithoutActivation();
                // SAFETY: Tauri owns this live NSWindow; all AppKit access is
                // confined to the main thread. No pointer escapes the callback.
                let native = unsafe { &*pointer.cast::<NSWindow>() };
                if native.isMiniaturized() {
                    native.deminiaturize(None);
                }
                native.makeKeyAndOrderFront(None);
                // Explicit tray/single-instance actions should activate the
                // app even when another app owns focus. This is not topmost.
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
                native.orderFrontRegardless();
            }
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        window.unminimize()?;
        window.show()?;
        window.set_focus()
    }
}

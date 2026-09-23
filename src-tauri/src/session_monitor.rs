use block2::RcBlock;
use objc2_app_kit::{NSWorkspace, NSWorkspaceSessionDidResignActiveNotification};
use objc2_foundation::NSNotification;
use std::ptr::NonNull;

/// Install the process-lifetime observer for macOS login-session resignation.
pub(crate) fn install() {
    let workspace = NSWorkspace::sharedWorkspace();
    let center = workspace.notificationCenter();
    let handler = RcBlock::new(|_: NonNull<NSNotification>| {
        if let Some(service) = crate::mcp_service() {
            service.on_system_session_resigned_active();
        }
    });

    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceSessionDidResignActiveNotification),
            None,
            None,
            &handler,
        )
    };

    // The notification center retains block-based observer tokens after
    // registration. This observer is installed once during app setup and is
    // intentionally kept alive until process exit, when the center is torn
    // down with the app.
    std::mem::forget(observer);
}

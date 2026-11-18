//! Launch-at-login via `SMAppService.mainApp` (macOS 13+).

use objc2_service_management::{SMAppService, SMAppServiceStatus};
use zwisper_core::{Error, Result};

/// Register / unregister the running app bundle as a login item.
/// Only works from a signed `.app` bundle (not `cargo run`). Any thread.
pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    // SAFETY: plain ServiceManagement calls on the main-app service object.
    unsafe {
        let service = SMAppService::mainAppService();
        let status = service.status();
        let res = if enabled {
            if status == SMAppServiceStatus::Enabled {
                return Ok(());
            }
            service.registerAndReturnError()
        } else {
            if status == SMAppServiceStatus::NotRegistered || status == SMAppServiceStatus::NotFound {
                return Ok(());
            }
            service.unregisterAndReturnError()
        };
        res.map_err(|e| Error::InvalidState(format!("login item: {}", e.localizedDescription())))
    }
}

/// Whether the app is currently registered to launch at login.
/// `RequiresApproval` (user must allow it in Login Items settings) counts as false.
pub fn launch_at_login_enabled() -> bool {
    // SAFETY: read-only status query.
    unsafe { SMAppService::mainAppService().status() == SMAppServiceStatus::Enabled }
}

/// Opens System Settings → General → Login Items.
pub fn open_login_items_settings() {
    // SAFETY: no arguments.
    unsafe { SMAppService::openSystemSettingsLoginItems() }
}

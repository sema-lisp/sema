#[cfg(feature = "nono")]
use nono::{AccessMode, CapabilitySet, Sandbox};

/// Applies an OS-level capability sandbox using `nono`.
pub fn apply_os_sandbox(sandbox: &crate::sandbox::Sandbox) -> Result<(), String> {
    #[cfg(not(feature = "nono"))]
    {
        let _ = sandbox;
        Ok(())
    }

    #[cfg(feature = "nono")]
    {
        if !Sandbox::is_supported() {
            return Ok(());
        }

        let mut os_caps = CapabilitySet::new();

        let fs_read = !sandbox.denied.contains(crate::sandbox::Caps::FS_READ);
        let fs_write = !sandbox.denied.contains(crate::sandbox::Caps::FS_WRITE);
        let network = !sandbox.denied.contains(crate::sandbox::Caps::NETWORK);

        if fs_read && fs_write {
            os_caps = os_caps
                .allow_path("/", AccessMode::ReadWrite)
                .map_err(|e| e.to_string())?;
        } else if fs_read {
            os_caps = os_caps
                .allow_path("/", AccessMode::Read)
                .map_err(|e| e.to_string())?;
        } else if fs_write {
            os_caps = os_caps
                .allow_path("/", AccessMode::Write)
                .map_err(|e| e.to_string())?;
        }

        if !network {
            os_caps = os_caps.block_network();
        }

        Sandbox::apply_auto(&os_caps).map_err(|e| format!("Sandbox error: {}", e))?;

        Ok(())
    }
}

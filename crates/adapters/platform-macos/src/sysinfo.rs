//! Host information used to pick a model tier.

/// Total physical memory in bytes (`sysctl hw.memsize` on macOS,
/// `sysconf(_SC_PHYS_PAGES) * page size` elsewhere). `0` if unknown.
pub fn system_memory_bytes() -> u64 {
    imp()
}

#[cfg(target_os = "macos")]
fn imp() -> u64 {
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: "hw.memsize" is a NUL-terminated name and yields a u64; out-pointers are valid.
    let rc = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            &mut value as *mut u64 as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 { value } else { 0 }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn imp() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let (pages, page) = unsafe { (libc::sysconf(libc::_SC_PHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
    if pages > 0 && page > 0 { pages as u64 * page as u64 } else { 0 }
}

#[cfg(not(unix))]
fn imp() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn memory_is_plausible() {
        let m = super::system_memory_bytes();
        assert!(m > 256 * 1024 * 1024, "{m}");
    }
}

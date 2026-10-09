//! The decode memory budget a viewer starts with, and how startup states it.

use dcmview::pixels::DecodeLimits;

/// The limits the viewer runs with and where they came from, for the
/// startup line: `flag` (`--decode-memory`) exactly as given, or else the
/// default for this machine's physical memory.
pub(super) fn decode_limits(flag: Option<DecodeLimits>) -> (DecodeLimits, &'static str) {
    match flag {
        Some(limits) => (limits, "set by --decode-memory"),
        None => {
            let physical = physical_memory_bytes();
            let source = if physical.is_some() {
                "default for this machine"
            } else {
                "default; this machine's physical memory could not be read"
            };
            (DecodeLimits::default_for(physical), source)
        }
    }
}

/// The physical memory of the machine in bytes, or `None` where it cannot
/// be read: on Linux and macOS the number of physical pages times the page
/// size. It is the machine's memory, not a container's or a cgroup's limit.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn physical_memory_bytes() -> Option<u64> {
    // SAFETY: `sysconf` takes an integer name, reads no memory of ours and
    // answers -1 for a name it does not support.
    let (pages, page_size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGE_SIZE),
        )
    };
    let pages = u64::try_from(pages).ok().filter(|pages| *pages > 0)?;
    let page_size = u64::try_from(page_size).ok().filter(|size| *size > 0)?;
    pages.checked_mul(page_size)
}

/// Other platforms, Windows among them: not read, so the default is the
/// flat [`dcmview::pixels::DECODE_MEMORY_DEFAULT_BYTES`].
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn physical_memory_bytes() -> Option<u64> {
    None
}

/// `bytes` the way `--decode-memory` accepts it when it is a whole number
/// of GiB or MiB (`4GiB`, `1536MiB`), and otherwise in bytes with the MiB
/// it is nearest below.
pub(super) fn byte_size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes > 0 && bytes.is_multiple_of(GIB) {
        format!("{}GiB", bytes / GIB)
    } else if bytes > 0 && bytes.is_multiple_of(MIB) {
        format!("{}MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes, about {}MiB", bytes / MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcmview::pixels::{DECODE_MEMORY_DEFAULT_BYTES, DECODE_MEMORY_DEFAULT_FLOOR_BYTES};

    #[test]
    fn a_budget_from_the_flag_is_used_as_given() {
        let given = DecodeLimits::with_memory(300 * 1024 * 1024).expect("above the minimum");
        assert_eq!(
            decode_limits(Some(given)),
            (given, "set by --decode-memory")
        );
    }

    #[test]
    fn the_default_budget_is_within_the_bounds_on_any_machine() {
        let (limits, source) = decode_limits(None);
        assert!(
            (DECODE_MEMORY_DEFAULT_FLOOR_BYTES..=DECODE_MEMORY_DEFAULT_BYTES)
                .contains(&limits.memory_bytes),
            "{limits:?}"
        );
        assert!(source.starts_with("default"), "{source}");
        assert_eq!(
            DecodeLimits {
                memory_bytes: DECODE_MEMORY_DEFAULT_BYTES,
                ..limits
            },
            DecodeLimits::DEFAULT
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn physical_memory_is_read_on_linux_and_macos() {
        let bytes = physical_memory_bytes().expect("physical memory");
        assert!(bytes >= 64 * 1024 * 1024, "{bytes}");
    }

    #[test]
    fn byte_sizes_are_written_as_the_flag_reads_them() {
        const MIB: u64 = 1024 * 1024;
        for (bytes, text) in [
            (4096 * MIB, "4GiB"),
            (1024 * MIB, "1GiB"),
            (1536 * MIB, "1536MiB"),
            (256 * MIB, "256MiB"),
            (2040 * MIB + 1, "2139095041 bytes, about 2040MiB"),
        ] {
            assert_eq!(byte_size(bytes), text);
        }
    }
}

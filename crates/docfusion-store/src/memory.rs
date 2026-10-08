//! What this machine has to work with.
//!
//! Asked rather than assumed, because the same build runs on a laptop and on a
//! workstation and the right answer differs by an order of magnitude.

use std::num::NonZero;

/// What one page being read costs, in bytes.
///
/// Measured rather than derived from the model files: a loaded pipeline is
/// about 250 MB resident, where the weights it holds measure 130 MB, because a
/// session's working memory scales with the page it is reading. Rounded up,
/// because being wrong on the low side means a machine that swaps.
const READER_BYTES: u64 = 256 * 1024 * 1024;

/// The share of the machine a knowledge base may use.
///
/// A quarter. Enough that reading a document is not slow, little enough that
/// everything else the person is doing still has room — and reading a document
/// is not the only thing happening on their computer.
const SHARE: u64 = 4;

/// How many pages this machine should read at once.
///
/// The machine's memory decides, not its core count, because the cost here is
/// memory: four readers cost a gigabyte whatever the processor can schedule.
/// Cores cap it, since more readers than cores is only more memory for the same
/// throughput.
pub fn page_concurrency() -> usize {
    let cores = cores();
    match total() {
        Some(total) => from_memory(total, cores),
        // A machine that will not say how much memory it has gets the cautious
        // answer rather than the generous one.
        None => cores.min(2),
    }
}

/// The arithmetic, on its own so it can be checked without having a machine of
/// a particular size to hand.
fn from_memory(total: u64, cores: usize) -> usize {
    let affordable = (total / SHARE / READER_BYTES) as usize;
    affordable.clamp(1, cores.max(1))
}

fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, NonZero::get)
}

/// Total physical memory in bytes, when the platform will say.
///
/// `None` is a real answer rather than a failure: a platform that will not say
/// gets a cautious default instead of a guessed number.
pub fn total() -> Option<u64> {
    platform_total()
}

#[cfg(windows)]
fn platform_total() -> Option<u64> {
    // The same shape as the file-replacement call in `atomic`: a small, direct
    // binding rather than a dependency for one function.
    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        memory_load: u32,
        total_physical: u64,
        available_physical: u64,
        total_page_file: u64,
        available_page_file: u64,
        total_virtual: u64,
        available_virtual: u64,
        available_extended_virtual: u64,
    }

    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatus) -> i32;
    }

    let mut status = MemoryStatus {
        length: size_of::<MemoryStatus>() as u32,
        memory_load: 0,
        total_physical: 0,
        available_physical: 0,
        total_page_file: 0,
        available_page_file: 0,
        total_virtual: 0,
        available_virtual: 0,
        available_extended_virtual: 0,
    };

    // SAFETY: the struct is `repr(C)` with the layout the call documents, and
    // `length` is set to its size as the call requires. It writes only into the
    // struct it was handed.
    let answered = unsafe { GlobalMemoryStatusEx(&mut status) };

    (answered != 0 && status.total_physical > 0).then_some(status.total_physical)
}

#[cfg(not(windows))]
fn platform_total() -> Option<u64> {
    // Linux and anything else that keeps its vitals in the same place. A system
    // that does not have this file gets `None`, which is a supported answer.
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;

    let kilobytes: u64 = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;

    Some(kilobytes * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn a_roomy_machine_is_capped_by_its_cores() {
        // Sixteen gigabytes is a four-gigabyte budget, which is sixteen
        // readers; a sixteen-core machine can run them.
        assert_eq!(from_memory(16 * GB, 16), 16);
    }

    #[test]
    fn a_small_machine_is_capped_by_its_memory() {
        // Eight gigabytes is two, which is eight readers; four cores cannot
        // use more than four, and more than four would cost memory for nothing.
        assert_eq!(from_memory(8 * GB, 4), 4);
        assert_eq!(from_memory(2 * GB, 8), 2);
    }

    /// Reading a document is not the only thing happening on the machine, so
    /// the answer is a share of it rather than all of it — and never zero, which
    /// would read nothing at all.
    #[test]
    fn a_crowded_machine_still_reads_one_page_at_a_time() {
        assert_eq!(from_memory(GB / 2, 8), 1);
        assert_eq!(from_memory(0, 8), 1);
    }

    #[test]
    fn the_answer_is_never_more_than_the_cores_or_less_than_one() {
        for total in [0, GB / 4, GB, 4 * GB, 64 * GB, 1024 * GB] {
            for cores in [1, 2, 8, 64] {
                let readers = from_memory(total, cores);
                assert!(readers >= 1, "{total} bytes, {cores} cores");
                assert!(readers <= cores, "{total} bytes, {cores} cores");
            }
        }
    }

    /// The call has to actually reach the operating system and come back with
    /// something plausible, or every machine silently gets the cautious default.
    #[test]
    fn this_machine_reports_a_believable_amount_of_memory() {
        let total = total().expect("a desktop operating system will say");

        assert!(
            total > 256 * 1024 * 1024,
            "less than a quarter gigabyte is not a computer: {total} bytes"
        );
        assert!(
            total < 64 * 1024 * GB,
            "more than sixty-four terabytes is a wrong number, not a big machine: {total} bytes"
        );
    }
}

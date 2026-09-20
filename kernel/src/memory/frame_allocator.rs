use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use aether_bootinfo::{BootInfo, MemoryRegionKind};
use spin::Once;

const FRAME_SIZE: u64 = 4096;
const MAX_USABLE_RANGES: usize = 32;

static ALLOCATOR: Once<FrameAllocator> = Once::new();
static USABLE_FRAME_COUNT: AtomicUsize = AtomicUsize::new(0);
static USABLE_RANGE_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn initialize(boot_info: &BootInfo) {
    let mut ranges = [UsableFrameRange::EMPTY; MAX_USABLE_RANGES];
    let mut range_count = 0usize;
    let mut total_frames = 0usize;

    for region in boot_info.memory_regions.iter().take(
        boot_info
            .memory_map_entries
            .min(boot_info.memory_regions.len()),
    ) {
        if region.kind != MemoryRegionKind::Usable || region.length < FRAME_SIZE {
            continue;
        }

        let aligned_start = align_up(region.base, FRAME_SIZE);
        let aligned_end = align_down(region.end(), FRAME_SIZE);

        if aligned_end <= aligned_start {
            continue;
        }

        if range_count < MAX_USABLE_RANGES {
            ranges[range_count] = UsableFrameRange::new(aligned_start, aligned_end);
            range_count += 1;
        }

        total_frames += ((aligned_end - aligned_start) / FRAME_SIZE) as usize;
    }

    USABLE_FRAME_COUNT.store(total_frames, Ordering::Release);
    USABLE_RANGE_COUNT.store(range_count, Ordering::Release);
    ALLOCATOR.call_once(|| FrameAllocator::new(ranges, range_count));
}

pub fn allocate_frame() -> Option<u64> {
    ALLOCATOR.get().and_then(FrameAllocator::allocate_frame)
}

pub fn usable_frame_count() -> usize {
    USABLE_FRAME_COUNT.load(Ordering::Acquire)
}

pub fn usable_range_count() -> usize {
    USABLE_RANGE_COUNT.load(Ordering::Acquire)
}

fn align_up(value: u64, align: u64) -> u64 {
    if value.is_multiple_of(align) {
        value
    } else {
        value + (align - (value % align))
    }
}

fn align_down(value: u64, align: u64) -> u64 {
    value - (value % align)
}

pub struct FrameAllocator {
    ranges: [UsableFrameRange; MAX_USABLE_RANGES],
    range_count: usize,
    current_range: AtomicUsize,
    current_frame: AtomicU64,
}

impl FrameAllocator {
    fn new(ranges: [UsableFrameRange; MAX_USABLE_RANGES], range_count: usize) -> Self {
        let initial_start = if range_count > 0 { ranges[0].start } else { 0 };

        Self {
            ranges,
            range_count,
            current_range: AtomicUsize::new(0),
            current_frame: AtomicU64::new(initial_start),
        }
    }

    fn allocate_frame(&self) -> Option<u64> {
        loop {
            let range_index = self.current_range.load(Ordering::Acquire);

            if range_index >= self.range_count {
                return None;
            }

            let range = self.ranges[range_index];
            let current = self.current_frame.load(Ordering::Acquire);

            if current < range.start || current >= range.end {
                if !self.advance_range(range_index) {
                    return None;
                }
                continue;
            }

            let next = current + FRAME_SIZE;

            if next > range.end {
                if !self.advance_range(range_index) {
                    return (current + FRAME_SIZE <= range.end).then_some(current);
                }
                continue;
            }

            match self.current_frame.compare_exchange(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(current),
                Err(_) => continue,
            }
        }
    }

    fn advance_range(&self, range_index: usize) -> bool {
        let next_range = range_index + 1;

        if next_range >= self.range_count {
            self.current_range
                .store(self.range_count, Ordering::Release);
            return false;
        }

        if self
            .current_range
            .compare_exchange(range_index, next_range, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.current_frame
                .store(self.ranges[next_range].start, Ordering::Release);
        }

        true
    }
}

#[derive(Clone, Copy)]
struct UsableFrameRange {
    start: u64,
    end: u64,
}

impl UsableFrameRange {
    const EMPTY: Self = Self { start: 0, end: 0 };

    const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }
}

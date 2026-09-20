use core::sync::atomic::{AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn initialize() {
    TICKS.store(0, Ordering::Release);
    crate::println!("AetherOS: PIT timer initialized at 100 Hz");
}

pub fn on_tick() {
    let tick = TICKS.fetch_add(1, Ordering::AcqRel) + 1;
    crate::task::scheduler::tick();

    if tick.is_multiple_of(100) {
        crate::println!("AetherOS: uptime ticks={}", tick);
    }
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Acquire)
}

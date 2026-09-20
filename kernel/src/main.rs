#![no_std]
#![no_main]

use core::panic::PanicInfo;

static INITIAL_MXCSR: u32 = 0x1f80;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    unsafe {
        core::arch::asm!(
            "mov {scratch}, cr0",
            "and {scratch}, -13",
            "or {scratch}, 2",
            "mov cr0, {scratch}",
            "mov {scratch}, cr4",
            "or {scratch}, 0x600",
            "mov cr4, {scratch}",
            "fninit",
            "ldmxcsr [{mxcsr}]",
            scratch = out(reg) _,
            mxcsr = in(reg) &INITIAL_MXCSR,
            options(nostack),
        );
    }

    aether_kernel::arch::x86_64::boot::enter()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    aether_kernel::core::panic::handle(info)
}

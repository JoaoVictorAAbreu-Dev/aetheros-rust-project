ENTRY(_start)

SECTIONS
{
    . = 0xffffffff80000000;

    .text : ALIGN(4K) {
        *(.text .text.*)
    }

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4K) {
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end))
        *(.data .data.*)
    }

    .bss : ALIGN(4K) {
        *(COMMON)
        *(.bss .bss.*)
    }
}

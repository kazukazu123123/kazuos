ENTRY(_start)

SECTIONS {
    . = 0x100000;

    .text : {
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
    }

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }

    /* Reserve 1MB stack at the end of the kernel image */
    . = ALIGN(16);
    __stack_bottom = .;
    . += 0x100000;
    __stack_top = .;
}

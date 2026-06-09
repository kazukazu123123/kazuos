#![no_std]

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub base: *mut u8,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub pixel_format: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryMapEntry {
    pub phys_start: u64,
    pub page_count: u64,
    pub ty: u32,
}

#[repr(C)]
#[derive(Debug)]
pub struct BootInfo {
    pub framebuffer: FramebufferInfo,
    pub font_data: *const u8,
    pub font_size: usize,
    pub heap_start: *mut u8,
    pub heap_size: usize,
    pub memory_map: *const MemoryMapEntry,
    pub memory_map_entries: usize,
    pub kernel_start: u64,
    pub kernel_end: u64,
    pub rsdp: u64,
    pub command_line: *const u8,
    pub command_line_len: usize,
    pub initrd_data: *const u8,
    pub initrd_size: usize,
}

impl BootInfo {
    pub fn font_slice(&self) -> &'static [u8] {
        unsafe { core::slice::from_raw_parts(self.font_data, self.font_size) }
    }

    pub fn memory_map_slice(&self) -> &'static [MemoryMapEntry] {
        unsafe { core::slice::from_raw_parts(self.memory_map, self.memory_map_entries) }
    }

    pub fn command_line(&self) -> &'static str {
        let bytes =
            unsafe { core::slice::from_raw_parts(self.command_line, self.command_line_len) };
        core::str::from_utf8(bytes).unwrap_or("")
    }

    pub fn initrd_slice(&self) -> &'static [u8] {
        unsafe { core::slice::from_raw_parts(self.initrd_data, self.initrd_size) }
    }
}

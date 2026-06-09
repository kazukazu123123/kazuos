#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct KxeHeader {
    pub magic: [u8; 4],
    pub entry: u64,
    pub code_offset: u64,
    pub code_size: u64,
    pub flags: u32,
    pub reserved: u32,
}

include!(concat!(env!("OUT_DIR"), "/user_programs_generated.rs"));

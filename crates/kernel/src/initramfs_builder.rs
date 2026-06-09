use alloc::vec::Vec;

use crate::{assets, user_programs};

const MAGIC: &[u8; 4] = b"KFS\0";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 28;
const ENTRY_SIZE: usize = 32;
const FLAG_DIR: u32 = 0x1;
const FLAG_FILE: u32 = 0x2;

struct SourceFile {
    path: &'static str,
    data: &'static [u8],
    flags: u32,
}

const SOURCES: &[SourceFile] = &[
    SourceFile {
        path: "/",
        data: &[],
        flags: FLAG_DIR,
    },
    SourceFile {
        path: "/bin",
        data: &[],
        flags: FLAG_DIR,
    },
    SourceFile {
        path: "/bin/init.kxe",
        data: user_programs::INIT_KXE,
        flags: FLAG_FILE,
    },
    SourceFile {
        path: "/bin/shell.kxe",
        data: user_programs::SHELL_KXE,
        flags: FLAG_FILE,
    },
    SourceFile {
        path: "/bin/ps.kxe",
        data: user_programs::PS_KXE,
        flags: FLAG_FILE,
    },
    SourceFile {
        path: "/bin/ktop.kxe",
        data: user_programs::KTOP_KXE,
        flags: FLAG_FILE,
    },
    SourceFile {
        path: "/bin/drv_ac97.kxe",
        data: user_programs::DRV_AC97_KXE,
        flags: FLAG_FILE,
    },
    SourceFile {
        path: "/bin/wavtestplay.kxe",
        data: user_programs::WAVTESTPLAY_KXE,
        flags: FLAG_FILE,
    },
    SourceFile {
        path: "/dev",
        data: &[],
        flags: FLAG_DIR,
    },
    SourceFile {
        path: "/audio",
        data: &[],
        flags: FLAG_DIR,
    },
    SourceFile {
        path: "/audio/test.wav",
        data: assets::TEST_WAV,
        flags: FLAG_FILE,
    },
];

pub fn build() -> Vec<u8> {
    let paths_size = SOURCES.iter().map(|file| file.path.len()).sum::<usize>();
    let mut data_offset = HEADER_SIZE + ENTRY_SIZE * SOURCES.len() + paths_size;
    let data_size = SOURCES.iter().map(|file| file.data.len()).sum::<usize>();
    let mut image = Vec::with_capacity(data_offset + data_size);
    image.extend_from_slice(MAGIC);
    image.extend_from_slice(&VERSION.to_le_bytes());
    image.extend_from_slice(&(SOURCES.len() as u32).to_le_bytes());
    image.extend_from_slice(&[0u8; 16]);
    let mut path_offset = 0usize;
    for file in SOURCES {
        image.extend_from_slice(&(file.path.len() as u16).to_le_bytes());
        image.extend_from_slice(&(path_offset as u16).to_le_bytes());
        if file.flags & FLAG_FILE != 0 {
            image.extend_from_slice(&(data_offset as u64).to_le_bytes());
            image.extend_from_slice(&(file.data.len() as u64).to_le_bytes());
            data_offset += file.data.len();
        } else {
            image.extend_from_slice(&0u64.to_le_bytes());
            image.extend_from_slice(&0u64.to_le_bytes());
        }
        image.extend_from_slice(&file.flags.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        path_offset += file.path.len();
    }
    for file in SOURCES {
        image.extend_from_slice(file.path.as_bytes());
    }
    for file in SOURCES {
        if file.flags & FLAG_FILE != 0 {
            image.extend_from_slice(file.data);
        }
    }
    image
}

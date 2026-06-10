use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    let user_programs_dir = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .join("user_programs");
    let link_ld = user_programs_dir.join("link.ld");

    if !link_ld.exists() {
        panic!("link.ld not found at {}", link_ld.display());
    }

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let sysroot = get_sysroot(&rustc);
    let libdir = format!("{}/lib/rustlib/x86_64-unknown-none/lib", sysroot);
    let objcopy = find_objcopy();

    let mut entries: Vec<_> = std::fs::read_dir(&user_programs_dir)
        .expect("failed to read user_programs dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name_str = name.to_str().unwrap_or("");
            // syscall_numbers.rs is shared via include!(), not compiled standalone
            if name_str == "syscall_numbers.rs" {
                return false;
            }
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s == "rs")
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut generated = String::new();
    let mut kxe_files: Vec<(String, Vec<u8>)> = Vec::new();

    for entry in &entries {
        let rs_file = entry.path();
        let stem = rs_file.file_stem().unwrap().to_str().unwrap().to_string();
        let const_name = format!("{}_KXE", stem.to_uppercase());

        let elf_path = Path::new(&out_dir).join(format!("{}.elf", stem));
        let bin_path = Path::new(&out_dir).join(format!("{}.bin", stem));

        let status = Command::new(&rustc)
            .args([
                "--edition",
                "2021",
                "--crate-type",
                "bin",
                "--target",
                "x86_64-unknown-none",
                "-L",
                &libdir,
                "-C",
                "panic=abort",
                "-C",
                &format!("link-arg=-T{}", link_ld.display()),
                "-C",
                "opt-level=3",
                "-o",
                &elf_path.to_string_lossy(),
            ])
            .arg(&rs_file)
            .status()
            .expect("failed to run rustc");

        if !status.success() {
            panic!("{}.rs compilation failed", stem);
        }

        let elf = std::fs::read(&elf_path).expect("failed to read elf");
        let relocs = find_relocations(&elf, 0x8000000000);
        let mem_size = elf_load_mem_size(&elf);

        let status = Command::new(&objcopy)
            .args([
                "-O",
                "binary",
                &elf_path.to_string_lossy(),
                &bin_path.to_string_lossy(),
            ])
            .status()
            .expect("failed to run objcopy");

        if !status.success() {
            panic!("objcopy failed for {}", stem);
        }

        let entry = read_elf_entry(&elf);
        let mut code = std::fs::read(&bin_path).expect("failed to read bin");
        // Pad binary to cover BSS (objcopy omits SHT_NOBITS sections)
        if mem_size > code.len() as u64 {
            code.resize(mem_size as usize, 0);
        }
        for &(offset, value) in &relocs {
            if offset + 8 <= code.len() as u64 {
                let start = offset as usize;
                code[start..start + 8].copy_from_slice(&value.to_le_bytes());
            }
        }

        let flags = if stem.starts_with("drv_") { 1u32 } else { 0u32 };
        let kxe = build_kxe(&code, entry, flags);

        let bytes: Vec<String> = kxe.iter().map(|b| format!("0x{:02x}", b)).collect();
        generated.push_str(&format!(
            "pub const {}: &[u8] = &[\n    {}\n];\n",
            const_name,
            bytes
                .chunks(16)
                .map(|chunk| chunk.join(", "))
                .collect::<Vec<_>>()
                .join(",\n    ")
        ));

        kxe_files.push((stem, kxe));

        println!("cargo:rerun-if-changed={}", rs_file.display());
    }

    println!("cargo:rerun-if-changed={}", link_ld.display());

    let gen_path = Path::new(&out_dir).join("user_programs_generated.rs");
    std::fs::write(&gen_path, generated).expect("failed to write user_programs_generated.rs");

    let workspace_root = Path::new(&manifest_dir).parent().unwrap().parent().unwrap();
    let wav_path = workspace_root.join("test.wav");
    println!("cargo:rerun-if-changed={}", wav_path.display());
    let wav_data = std::fs::read(&wav_path).unwrap_or_default();

    let kfs = build_kfs(&kxe_files, &wav_data);
    let initrd_path = workspace_root.join("target").join("initrd.kfs");
    std::fs::create_dir_all(initrd_path.parent().unwrap()).ok();
    std::fs::write(&initrd_path, &kfs).expect("failed to write initrd.kfs");
}

fn get_sysroot(rustc: &str) -> String {
    let out = Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .expect("failed to get sysroot");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

const R_X86_64_RELATIVE: u64 = 8;

/// Read ELF64 e_entry (the virtual address of the program's entry point).
fn read_elf_entry(elf: &[u8]) -> u64 {
    if elf.len() < 32 || &elf[0..4] != b"\x7fELF" || elf[4] != 2 {
        return 0;
    }
    read_u64(&elf[24..32])
}

/// Return the total virtual memory footprint of all PT_LOAD segments (including BSS).
fn elf_load_mem_size(elf: &[u8]) -> u64 {
    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" || elf[4] != 2 {
        return 0;
    }
    let phoff     = read_u64(&elf[32..40]) as usize;
    let phentsize = read_u16(&elf[54..56]) as usize;
    let phnum     = read_u16(&elf[56..58]) as usize;
    let mut max_end = 0u64;
    for i in 0..phnum {
        let off = phoff + i * phentsize;
        if off + 56 > elf.len() { break; }
        let p_type  = read_u32(&elf[off..]);
        if p_type != 1 { continue; } // PT_LOAD only
        let p_vaddr  = read_u64(&elf[off + 16..]);
        let p_memsz  = read_u64(&elf[off + 40..]);
        let end = p_vaddr + p_memsz;
        if end > max_end { max_end = end; }
    }
    max_end
}

fn find_relocations(elf: &[u8], user_base: u64) -> Vec<(u64, u64)> {
    let mut results = Vec::new();

    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" || elf[4] != 2 {
        return results;
    }

    let shoff = read_u64(&elf[40..48]);
    let shentsize = read_u16(&elf[58..60]) as u64;
    let shnum = read_u16(&elf[60..62]) as usize;
    let shentsize_min = 64;

    if shoff == 0 || shentsize < shentsize_min || shnum == 0 {
        return results;
    }

    let shstrndx = read_u16(&elf[62..64]) as usize;
    let (strtab_off, strtab_size) = if shstrndx < shnum {
        let s = read_section_header(elf, shoff, shentsize, shstrndx);
        (s.3, s.4)
    } else {
        (0, 0)
    };

    for i in 0..shnum {
        let s = read_section_header(elf, shoff, shentsize, i);
        let name = read_str(elf, strtab_off, strtab_size, s.0 as usize);
        let sh_offset = s.3;
        let sh_size = s.4;

        if name == ".rela.dyn" && sh_size >= 24 {
            let mut off = 0;
            while off + 24 <= sh_size {
                let entry_off = sh_offset + off;
                if entry_off + 24 > elf.len() as u64 {
                    break;
                }
                let r_offset = read_u64(&elf[entry_off as usize..][..8]);
                let r_info = read_u64(&elf[entry_off as usize + 8..][..8]);
                let r_addend = read_i64(&elf[entry_off as usize + 16..][..8]);
                let r_type = r_info & 0xffffffff;
                if r_type == R_X86_64_RELATIVE {
                    results.push((r_offset, user_base.wrapping_add(r_addend as u64)));
                }
                off += 24;
            }
        }
    }

    results
}

fn read_section_header(
    elf: &[u8],
    shoff: u64,
    shentsize: u64,
    idx: usize,
) -> (u32, u64, u64, u64, u64) {
    let offset = shoff + (idx as u64) * shentsize;
    if offset + 64 > elf.len() as u64 {
        return (0, 0, 0, 0, 0);
    }
    let data = &elf[offset as usize..];
    let sh_name = read_u32(&data[0..4]);
    let sh_addr = read_u64(&data[16..24]);
    let sh_file_offset = read_u64(&data[24..32]);
    let sh_size = read_u64(&data[32..40]);
    (sh_name, 0, sh_addr, sh_file_offset, sh_size)
}

fn read_str(elf: &[u8], strtab_off: u64, strtab_size: u64, name_off: usize) -> String {
    let pos = strtab_off + name_off as u64;
    let end = strtab_off + strtab_size;
    if pos as usize >= elf.len() || pos >= end {
        return String::new();
    }
    let mut s = Vec::new();
    for &byte in &elf[pos as usize..(end as usize).min(elf.len())] {
        if byte == 0 {
            break;
        }
        s.push(byte);
    }
    String::from_utf8_lossy(&s).into_owned()
}

fn read_u32(data: &[u8]) -> u32 {
    u32::from_le_bytes(data[..4].try_into().unwrap())
}

fn read_u64(data: &[u8]) -> u64 {
    u64::from_le_bytes(data[..8].try_into().unwrap())
}

fn read_i64(data: &[u8]) -> i64 {
    i64::from_le_bytes(data[..8].try_into().unwrap())
}

fn read_u16(data: &[u8]) -> u16 {
    u16::from_le_bytes(data[..2].try_into().unwrap())
}

fn find_objcopy() -> String {
    for name in &["llvm-objcopy", "objcopy"] {
        if Command::new(name)
            .arg("--version")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return name.to_string();
        }
    }
    panic!("objcopy not found. Please install llvm-objcopy or GNU objcopy.");
}

fn build_kfs(kxe_files: &[(String, Vec<u8>)], wav_data: &[u8]) -> Vec<u8> {
    const MAGIC: &[u8; 4] = b"KFS\0";
    const VERSION: u32 = 1;
    const HEADER_SIZE: usize = 28;
    const ENTRY_SIZE: usize = 32;
    const FLAG_DIR: u32 = 0x1;
    const FLAG_FILE: u32 = 0x2;

    struct Entry {
        path: &'static str,
        data: Vec<u8>,
        flags: u32,
    }

    let mut sources: Vec<Entry> = vec![
        Entry { path: "/", data: vec![], flags: FLAG_DIR },
        Entry { path: "/bin", data: vec![], flags: FLAG_DIR },
    ];
    for (stem, kxe) in kxe_files {
        sources.push(Entry {
            path: Box::leak(format!("/bin/{}.kxe", stem).into_boxed_str()),
            data: kxe.clone(),
            flags: FLAG_FILE,
        });
    }
    sources.push(Entry { path: "/audio", data: vec![], flags: FLAG_DIR });
    if !wav_data.is_empty() {
        sources.push(Entry { path: "/audio/test.wav", data: wav_data.to_vec(), flags: FLAG_FILE });
    }

    let paths_size: usize = sources.iter().map(|e| e.path.len()).sum();
    let mut data_offset = HEADER_SIZE + ENTRY_SIZE * sources.len() + paths_size;
    let mut image = Vec::new();
    image.extend_from_slice(MAGIC);
    image.extend_from_slice(&VERSION.to_le_bytes());
    image.extend_from_slice(&(sources.len() as u32).to_le_bytes());
    image.extend_from_slice(&[0u8; 16]);

    let mut path_offset = 0usize;
    for entry in &sources {
        image.extend_from_slice(&(entry.path.len() as u16).to_le_bytes());
        image.extend_from_slice(&(path_offset as u16).to_le_bytes());
        if entry.flags & FLAG_FILE != 0 {
            image.extend_from_slice(&(data_offset as u64).to_le_bytes());
            image.extend_from_slice(&(entry.data.len() as u64).to_le_bytes());
            data_offset += entry.data.len();
        } else {
            image.extend_from_slice(&0u64.to_le_bytes());
            image.extend_from_slice(&0u64.to_le_bytes());
        }
        image.extend_from_slice(&entry.flags.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        path_offset += entry.path.len();
    }
    for entry in &sources {
        image.extend_from_slice(entry.path.as_bytes());
    }
    for entry in &sources {
        if entry.flags & FLAG_FILE != 0 {
            image.extend_from_slice(&entry.data);
        }
    }
    image
}

fn build_kxe(code: &[u8], entry: u64, flags: u32) -> Vec<u8> {
    let header_size = 36;
    let code_offset = header_size as u64;
    let mut result = Vec::with_capacity(header_size + code.len());
    result.extend_from_slice(b"KXE\0");
    result.extend_from_slice(&entry.to_le_bytes());
    result.extend_from_slice(&code_offset.to_le_bytes());
    result.extend_from_slice(&(code.len() as u64).to_le_bytes());
    result.extend_from_slice(&flags.to_le_bytes()); // flags
    result.extend_from_slice(&0u32.to_le_bytes()); // reserved
    result.extend_from_slice(code);
    result
}

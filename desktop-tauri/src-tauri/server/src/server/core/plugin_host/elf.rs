//! ELF 入口的最小只读检查：定位 `PT_INTERP` 指向的动态解释器路径。
//!
//! 只服务插件宿主的排障文案（`lifecycle` 的 spawn 失败分类）：spawn 报
//! ENOENT 而入口文件明明在时，把「解释器不存在」从一堆同码错误里摘出来。
//! 只解析 32/64 位**小端** ELF 的程序头表，不引入任何依赖；读不出
//! （非 ELF / 损坏 / 大端 / 截断）一律返回 `None`，调用方退回泛化文案
//! —— 排障提示宁缺勿错，错一次就把用户带进沟里。

use std::path::Path;

const PT_INTERP: u32 = 3;
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
// 解释器路径就几十字节；上限只为防损坏文件给出荒唐的 p_filesz。
const INTERP_PATH_MAX: u64 = 4096;

/// 入口是 ELF 且其解释器文件不存在时，返回解释器路径；其余情况返回 `None`。
pub(super) fn missing_interpreter(exe: &Path) -> Option<String> {
    let path = interpreter(exe)?;
    if Path::new(&path).exists() {
        return None;
    }
    Some(path)
}

/// 读 `PT_INTERP` 段的解释器路径（按 NUL 截断）。非 ELF、大端、读不出 → `None`。
fn interpreter(exe: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(exe).ok()?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header).ok()?;
    if header[..4] != ELF_MAGIC || header[5] != 1 {
        // 魔数不对或不是小端（EI_DATA = ELFDATA2LSB）：不做判断，不硬猜
        return None;
    }
    // EI_CLASS 决定字段宽度：1 = 32 位（e_phoff 在 0x1C），2 = 64 位（0x20）。
    // 程序头内 p_offset / p_filesz 的偏移也随宽度走。
    let (phoff, phentsize, phnum, field_width, p_offset_at, p_filesz_at) = match header[4] {
        1 => (
            le(&header[0x1c..0x20])?, le(&header[0x2a..0x2c])?, le(&header[0x2c..0x2e])?,
            4usize, 0x04, 0x10,
        ),
        2 => (
            le(&header[0x20..0x28])?, le(&header[0x36..0x38])?, le(&header[0x38..0x3a])?,
            8usize, 0x08, 0x20,
        ),
        _ => return None,
    };
    if phentsize == 0 {
        return None;
    }
    for index in 0..phnum {
        let entry_at = phoff.checked_add(index.checked_mul(phentsize)?)?;
        file.seek(SeekFrom::Start(entry_at)).ok()?;
        let mut entry = vec![0u8; phentsize as usize];
        file.read_exact(&mut entry).ok()?;
        if le(&entry[..4])? != u64::from(PT_INTERP) {
            continue;
        }
        let offset = le(&entry[p_offset_at..p_offset_at + field_width])?;
        let size = le(&entry[p_filesz_at..p_filesz_at + field_width])?
            .min(INTERP_PATH_MAX);
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut path = vec![0u8; size as usize];
        file.read_exact(&mut path).ok()?;
        let text = String::from_utf8_lossy(&path);
        let text = text.trim_end_matches('\0').trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

/// 按小端读 2 / 4 / 8 字节（宽度由切片长度决定），统一升成 u64。
fn le(bytes: &[u8]) -> Option<u64> {
    if bytes.len() > 8 {
        return None;
    }
    Some(bytes.iter().rev().fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手拼一个 64 位小端 ELF：只有一条 PT_INTERP 程序头，路径与大小可配。
    fn elf64_with_interp(interp: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; 64 + 56 + 64];
        bytes[..4].copy_from_slice(&ELF_MAGIC);
        bytes[4] = 2; // 64 位
        bytes[5] = 1; // 小端
        let phoff = 64u64;
        bytes[0x20..0x28].copy_from_slice(&phoff.to_le_bytes());
        bytes[0x36..0x38].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        bytes[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
        let entry = &mut bytes[64..64 + 56];
        entry[..4].copy_from_slice(&PT_INTERP.to_le_bytes());
        entry[0x08..0x10].copy_from_slice(&(64u64 + 56).to_le_bytes()); // p_offset
        entry[0x20..0x28].copy_from_slice(&(interp.len() as u64 + 1).to_le_bytes()); // p_filesz 含 NUL
        let tail = &mut bytes[64 + 56..];
        tail[..interp.len()].copy_from_slice(interp.as_bytes());
        bytes
    }

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agent2api-elf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_missing_interpreter_is_reported_with_its_path() {
        let exe = write_temp("interp-missing.so", &elf64_with_interp("/no/such/ld-linux.so.2"));
        assert_eq!(
            missing_interpreter(&exe),
            Some("/no/such/ld-linux.so.2".to_string())
        );
    }

    #[test]
    fn an_existing_interpreter_is_not_reported() {
        let exe = write_temp("interp-exists.so", &elf64_with_interp("/bin/sh"));
        // /bin/sh 在任何测试环境都存在；解释器在 → 缺失判定不触发
        assert_eq!(missing_interpreter(&exe), None);
    }

    #[test]
    fn non_elf_files_are_not_judged() {
        let exe = write_temp("not-elf.bin", b"#! /bin/sh\necho hi\n");
        assert_eq!(missing_interpreter(&exe), None);
    }

    #[test]
    fn truncated_files_are_not_judged() {
        let dir = std::env::temp_dir().join(format!("agent2api-elf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("truncated.so");
        std::fs::write(&path, &ELF_MAGIC).unwrap();
        assert_eq!(missing_interpreter(&path), None);
    }
}

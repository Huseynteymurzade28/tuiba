//! `--trace-io`: which I/O registers to watch, and how a traced write
//! reads in the log.

use std::io::{self, Write};
use std::ops::Range;

use tuiba_core::memory::io_trace::{IoWrite, Writer};

/// I/O registers by their GBATEK names: offset into the I/O area and
/// size in bytes.
const REGISTERS: &[(&str, u32, u32)] = &[
    ("DISPCNT", 0x000, 2),
    ("GREENSWAP", 0x002, 2),
    ("DISPSTAT", 0x004, 2),
    ("VCOUNT", 0x006, 2),
    ("BG0CNT", 0x008, 2),
    ("BG1CNT", 0x00A, 2),
    ("BG2CNT", 0x00C, 2),
    ("BG3CNT", 0x00E, 2),
    ("BG0HOFS", 0x010, 2),
    ("BG0VOFS", 0x012, 2),
    ("BG1HOFS", 0x014, 2),
    ("BG1VOFS", 0x016, 2),
    ("BG2HOFS", 0x018, 2),
    ("BG2VOFS", 0x01A, 2),
    ("BG3HOFS", 0x01C, 2),
    ("BG3VOFS", 0x01E, 2),
    ("BG2PA", 0x020, 2),
    ("BG2PB", 0x022, 2),
    ("BG2PC", 0x024, 2),
    ("BG2PD", 0x026, 2),
    ("BG2X", 0x028, 4),
    ("BG2Y", 0x02C, 4),
    ("BG3PA", 0x030, 2),
    ("BG3PB", 0x032, 2),
    ("BG3PC", 0x034, 2),
    ("BG3PD", 0x036, 2),
    ("BG3X", 0x038, 4),
    ("BG3Y", 0x03C, 4),
    ("WIN0H", 0x040, 2),
    ("WIN1H", 0x042, 2),
    ("WIN0V", 0x044, 2),
    ("WIN1V", 0x046, 2),
    ("WININ", 0x048, 2),
    ("WINOUT", 0x04A, 2),
    ("MOSAIC", 0x04C, 2),
    ("BLDCNT", 0x050, 2),
    ("BLDALPHA", 0x052, 2),
    ("BLDY", 0x054, 2),
    ("SOUND1CNT_L", 0x060, 2),
    ("SOUND1CNT_H", 0x062, 2),
    ("SOUND1CNT_X", 0x064, 2),
    ("SOUND2CNT_L", 0x068, 2),
    ("SOUND2CNT_H", 0x06C, 2),
    ("SOUND3CNT_L", 0x070, 2),
    ("SOUND3CNT_H", 0x072, 2),
    ("SOUND3CNT_X", 0x074, 2),
    ("SOUND4CNT_L", 0x078, 2),
    ("SOUND4CNT_H", 0x07C, 2),
    ("SOUNDCNT_L", 0x080, 2),
    ("SOUNDCNT_H", 0x082, 2),
    ("SOUNDCNT_X", 0x084, 2),
    ("SOUNDBIAS", 0x088, 2),
    ("WAVE_RAM", 0x090, 16),
    ("FIFO_A", 0x0A0, 4),
    ("FIFO_B", 0x0A4, 4),
    ("DMA0SAD", 0x0B0, 4),
    ("DMA0DAD", 0x0B4, 4),
    ("DMA0CNT_L", 0x0B8, 2),
    ("DMA0CNT_H", 0x0BA, 2),
    ("DMA1SAD", 0x0BC, 4),
    ("DMA1DAD", 0x0C0, 4),
    ("DMA1CNT_L", 0x0C4, 2),
    ("DMA1CNT_H", 0x0C6, 2),
    ("DMA2SAD", 0x0C8, 4),
    ("DMA2DAD", 0x0CC, 4),
    ("DMA2CNT_L", 0x0D0, 2),
    ("DMA2CNT_H", 0x0D2, 2),
    ("DMA3SAD", 0x0D4, 4),
    ("DMA3DAD", 0x0D8, 4),
    ("DMA3CNT_L", 0x0DC, 2),
    ("DMA3CNT_H", 0x0DE, 2),
    ("TM0CNT_L", 0x100, 2),
    ("TM0CNT_H", 0x102, 2),
    ("TM1CNT_L", 0x104, 2),
    ("TM1CNT_H", 0x106, 2),
    ("TM2CNT_L", 0x108, 2),
    ("TM2CNT_H", 0x10A, 2),
    ("TM3CNT_L", 0x10C, 2),
    ("TM3CNT_H", 0x10E, 2),
    ("SIODATA32", 0x120, 4),
    ("SIOMULTI2", 0x124, 2),
    ("SIOMULTI3", 0x126, 2),
    ("SIOCNT", 0x128, 2),
    ("SIODATA8", 0x12A, 2),
    ("KEYINPUT", 0x130, 2),
    ("KEYCNT", 0x132, 2),
    ("RCNT", 0x134, 2),
    ("JOYCNT", 0x140, 2),
    ("JOY_RECV", 0x150, 4),
    ("JOY_TRANS", 0x154, 4),
    ("JOYSTAT", 0x158, 2),
    ("IE", 0x200, 2),
    ("IF", 0x202, 2),
    ("WAITCNT", 0x204, 2),
    ("IME", 0x208, 2),
    ("POSTFLG", 0x300, 1),
    ("HALTCNT", 0x301, 1),
];

/// Groups of registers that are usually watched together.
const GROUPS: &[(&str, Range<u32>)] = &[
    ("all", 0x000..0x400),
    ("lcd", 0x000..0x060),
    ("sound", 0x060..0x0B0),
    ("dma", 0x0B0..0x0E0),
    ("timers", 0x100..0x110),
    ("serial", 0x120..0x160),
    ("irq", 0x200..0x20A),
];

/// Parses a comma-separated watch list: register names (any case),
/// group names, or offsets/addresses in hex (`0x208`, `4000208`), and
/// returns the byte ranges to watch.
pub fn parse_watch(spec: &str) -> Option<Vec<Range<u32>>> {
    spec.split(',')
        .map(|item| {
            let item = item.trim();
            if let Some((_, range)) = GROUPS
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(item))
            {
                return Some(range.clone());
            }
            if let Some(&(_, offset, size)) = REGISTERS
                .iter()
                .find(|(name, ..)| name.eq_ignore_ascii_case(item))
            {
                return Some(offset..offset + size);
            }
            let digits = item
                .strip_prefix("0x")
                .or_else(|| item.strip_prefix("0X"))
                .unwrap_or(item);
            let address = u32::from_str_radix(digits, 16).ok()?;
            // Either an offset into the I/O area or a full address in it.
            let offset = match address {
                0..0x400 => address,
                0x0400_0000..0x0400_0400 => address - 0x0400_0000,
                _ => return None,
            };
            let size = REGISTERS
                .iter()
                .find(|&&(_, start, _)| start == offset)
                .map_or(2, |&(_, _, size)| size);
            Some(offset..offset + size)
        })
        .collect()
}

/// The name of the register at `offset`: its own, `NAME+n` inside a
/// larger one, or the bare offset where nothing is mapped.
fn register_name(offset: u32) -> String {
    match REGISTERS
        .iter()
        .find(|&&(_, start, size)| (start..start + size).contains(&offset))
    {
        Some(&(name, start, _)) if start == offset => name.to_string(),
        Some(&(name, start, _)) => format!("{name}+{}", offset - start),
        None => format!("{offset:#05x}"),
    }
}

/// Writes one line per traced write of frame `frame`.
pub fn write_log(mut out: impl Write, frame: u32, writes: &[IoWrite]) -> io::Result<()> {
    for write in writes {
        let value = match write.width {
            1 => format!("{:02x}", write.value),
            2 => format!("{:04x}", write.value),
            _ => format!("{:08x}", write.value),
        };
        let source = match write.writer {
            Writer::Cpu(pc) => format!("pc {pc:08x}"),
            Writer::Dma(n) => format!("dma{n}"),
        };
        writeln!(
            out,
            "io frame {frame} line {} cycle {}: {} <- {value} ({source})",
            write.line,
            write.cycle,
            register_name(write.offset),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The watch list as `(start, end)` pairs.
    fn watch(spec: &str) -> Option<Vec<(u32, u32)>> {
        parse_watch(spec).map(|ranges| ranges.iter().map(|r| (r.start, r.end)).collect())
    }

    #[test]
    fn watch_lists() {
        assert_eq!(watch("DISPCNT"), Some(vec![(0x000, 0x002)]));
        assert_eq!(
            watch("ime, bg2x,0x4000004"),
            Some(vec![(0x208, 0x20A), (0x028, 0x02C), (0x004, 0x006)])
        );
        assert_eq!(watch("Sound"), Some(vec![(0x060, 0x0B0)]));
        assert_eq!(watch("0x3fe"), Some(vec![(0x3FE, 0x400)]));
        assert_eq!(watch("nonsense"), None);
        assert_eq!(watch("0x5000000"), None);
        assert_eq!(watch("DISPCNT,"), None);
    }

    #[test]
    fn registers_are_sorted_and_disjoint() {
        for pair in REGISTERS.windows(2) {
            let ((a, start, size), (b, next, _)) = (pair[0], pair[1]);
            assert!(start + size <= next, "{a} overlaps {b}");
        }
    }

    #[test]
    fn log_lines() {
        let writes = [
            IoWrite {
                offset: 0x000,
                width: 2,
                value: 0x0403,
                writer: Writer::Cpu(0x0800_01A4),
                line: 160,
                cycle: 12,
            },
            IoWrite {
                offset: 0x02A,
                width: 2,
                value: 0x1,
                writer: Writer::Dma(0),
                line: 3,
                cycle: 1000,
            },
            IoWrite {
                offset: 0x0A0,
                width: 4,
                value: 0xDEAD_BEEF,
                writer: Writer::Dma(1),
                line: 0,
                cycle: 0,
            },
        ];
        let mut out = Vec::new();
        write_log(&mut out, 7, &writes).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "io frame 7 line 160 cycle 12: DISPCNT <- 0403 (pc 080001a4)\n\
             io frame 7 line 3 cycle 1000: BG2X+2 <- 0001 (dma0)\n\
             io frame 7 line 0 cycle 0: FIFO_A <- deadbeef (dma1)\n"
        );
        assert_eq!(register_name(0x3F0), "0x3f0");
    }
}

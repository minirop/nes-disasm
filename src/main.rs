use byteorder::BigEndian;
use byteorder::ReadBytesExt;
use clap::Parser;
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;

#[derive(Debug, Parser)]
struct Args {
    filename: String,

    #[arg(short, long)]
    cdl: String,

    #[arg(short, long)]
    output: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    disassemble(&args.filename, &args.cdl, &args.output)
}

const BANK_SIZE: usize = 0x4000;
const CHR_SIZE: usize = 0x2000;

#[derive(Copy, Clone)]
struct RomData {
    banks_count: u8,
    mapper: u8,
}

fn disassemble(filename: &str, cdl: &str, output: &str) -> Result<(), Box<dyn std::error::Error>> {
    let data: Vec<u8> = fs::read(&cdl)?;

    let mut rom = File::open(filename)?;

    let ines = rom.read_u32::<BigEndian>()?;
    if ines != 0x4E45531A {
        return Err(Box::new(Error::new(
            ErrorKind::InvalidInput,
            "This file is not an iNES ROM.",
        )));
    }

    let prg_banks_count = rom.read_u8()?;
    let chr_banks_count = rom.read_u8()?;
    let flags_06 = rom.read_u8()?;
    let mut padding = vec![0u8; 9];
    rom.read(&mut padding)?;
    let mapper = flags_06 >> 4;

    fs::create_dir_all(output)?;
    let mut output_file = File::create(format!("{output}/main.s"))?;

    writeln!(output_file, ".MEMORYMAP")?;
    writeln!(output_file, "    DEFAULTSLOT 1")?;
    writeln!(output_file, "    SLOTSIZE $0010")?;
    writeln!(output_file, "    SLOT 0 $0000")?;
    writeln!(output_file, "    SLOTSIZE ${BANK_SIZE:X}")?;
    writeln!(output_file, "    SLOT 1 $C000")?;
    writeln!(output_file, "    SLOTSIZE ${CHR_SIZE:X}")?;
    writeln!(output_file, "    SLOT 2 $0000")?;
    writeln!(output_file, "    SLOTSIZE $800")?;
    writeln!(output_file, "    SLOT 3 $0000")?;
    writeln!(output_file, ".ENDME\n")?;

    writeln!(output_file, ".ROMBANKMAP")?;
    writeln!(
        output_file,
        "    BANKSTOTAL {}",
        prg_banks_count + chr_banks_count + 1
    )?;
    writeln!(output_file, "    BANKSIZE $0010")?;
    writeln!(output_file, "    BANKS 1")?;
    writeln!(output_file, "    BANKSIZE ${BANK_SIZE:X}")?;
    writeln!(output_file, "    BANKS {prg_banks_count}")?;
    writeln!(output_file, "    BANKSIZE ${CHR_SIZE:X}")?;
    writeln!(output_file, "    BANKS {chr_banks_count}")?;
    writeln!(output_file, ".ENDRO\n")?;

    writeln!(output_file, ".BANK 0 SLOT 0")?;
    writeln!(output_file, ".ORG $0000\n")?;
    writeln!(output_file, ".SECTION \"Header\" FORCE\n")?;
    writeln!(output_file, ".db \"NES\", $1A")?;
    writeln!(output_file, ".db ${prg_banks_count:02X}")?;
    writeln!(output_file, ".db ${chr_banks_count:02X}")?;
    write!(output_file, ".db ${flags_06:02X}")?;
    for b in padding {
        write!(output_file, " ${b:02X}")?;
    }
    writeln!(output_file, "\n\n.ENDS\n")?;

    writeln!(output_file, ".RAMSECTION \"RAM\" SLOT 3")?;
    writeln!(output_file, ".ENDS\n")?;

    let rom_data = RomData {
        banks_count: prg_banks_count,
        mapper,
    };
    for id in 0..prg_banks_count {
        writeln!(output_file, ".INCLUDE \"bank{id:03}.asm\"")?;

        let mut bank = vec![0u8; BANK_SIZE];
        rom.read(&mut bank)?;

        let bank_offset = (id as usize) * BANK_SIZE;
        let cld_part = &data[bank_offset..bank_offset + BANK_SIZE];
        assert_eq!(cld_part.len(), BANK_SIZE);

        disassemble_prg_bank(id, bank, rom_data, cld_part, output)?;
    }

    for id in 0..chr_banks_count {
        writeln!(output_file, "\n.BANK {} SLOT 2", id + prg_banks_count + 1)?;
        writeln!(output_file, ".ORG $0000")?;
        writeln!(output_file, ".INCBIN \"bank{id:03}.chr\"")?;

        let mut bank = vec![0u8; CHR_SIZE];
        rom.read(&mut bank)?;
        fs::write(format!("{output}/bank{id:03}.chr"), bank)?;
    }

    Ok(())
}

fn disassemble_prg_bank(
    id: u8,
    bank: Vec<u8>,
    rom_data: RomData,
    cdl: &[u8],
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut buffer = vec![];

    let mut i = 0;
    let mut print_label = true;
    let mut labels = HashSet::new();
    let mut is_inside_data = false;

    let bank_offset = get_bank_offset(id, rom_data.banks_count, rom_data.mapper);
    while i < bank.len() {
        let g_offset = i + id as usize * 0x10000 + bank_offset;

        if (cdl[i] & 1) == 1 {
            // is code
            if is_inside_data {
                buffer.push((0, format!("; end of data")));
                is_inside_data = false;
            }

            // if (cdl[i] & 3) == 3 {
            // buffer.push((0, format!("; code AND data???")));
            // }

            let op = bank[i] as usize;
            if let Some(Some(opcode)) = OPCODES.get(op) {
                if print_label {
                    labels.insert(g_offset);
                    print_label = false;
                }

                let (size, output, target) =
                    write_addressing(&opcode.addressing, &bank[(i + 1)..], id, g_offset, rom_data)?;
                i += size;

                if let Some(addr) = target {
                    labels.insert(addr);
                }

                buffer.push((g_offset, format!("    {} {}", opcode.name, output)));

                if opcode.name == "RTS" || opcode.name == "JMP" {
                    buffer.push((0, "".into()));
                    print_label = true;
                }
            } else {
                buffer.push((g_offset, format!(".db ${op:02X} ; invalid opcode?")));
            }
        } else if (cdl[i] & 3) == 2 {
            // is data
            if !is_inside_data {
                buffer.push((0, format!("; start of data")));
                is_inside_data = true;
            }

            buffer.push((g_offset, format!(".db ${:02X}", bank[i])));
        } else {
            // is unknown
            if is_inside_data {
                buffer.push((0, format!("; end of data")));
                is_inside_data = false;
            }

            print_label = true;
            buffer.push((g_offset, format!(".db ${:02X}", bank[i])));
        }

        i += 1;
    }

    if is_inside_data {
        buffer.push((0, format!("; end of data")));
    }

    let mut output = File::create(format!("{path}/bank{id:03}.asm"))?;

    writeln!(output, ".BANK {}", id + 1)?;
    writeln!(output, ".ORG $0000\n")?;
    writeln!(output, ".SECTION \"Bank{id}\" FORCE\n")?;

    for (addr, s) in buffer {
        if labels.contains(&addr) {
            writeln!(output, "L{addr:06X}:")?;
        }
        writeln!(output, "{s}")?;
    }

    writeln!(output, "\n.ENDS")?;

    Ok(())
}

fn get_bank_offset(bank: u8, banks_count: u8, mapper: u8) -> usize {
    match mapper {
        10 => {
            if bank == banks_count - 1 {
                0xC000
            } else {
                0x8000
            }
        }
        _ => {
            println!("Unhandled mapper: {mapper}");
            0x8000
        }
    }
}

fn write_addressing(
    addressing: &Addressing,
    bank: &[u8],
    id: u8,
    position: usize,
    rom_data: RomData,
) -> Result<(usize, String, Option<usize>), Box<dyn std::error::Error>> {
    Ok(match addressing {
        Addressing::Absolute => {
            let (label, target) = get_target(id, bank[0], bank[1], rom_data);
            (2, label, Some(target))
        }
        Addressing::AbsoluteX => {
            let (label, target) = get_target(id, bank[0], bank[1], rom_data);
            (2, format!("{label},X"), Some(target))
        }
        Addressing::AbsoluteY => {
            let (label, target) = get_target(id, bank[0], bank[1], rom_data);
            (2, format!("{label},Y"), Some(target))
        }
        Addressing::Accumulator => (0, "".into(), None),
        Addressing::Immediate => (1, format!("#{}", bank[0]), None),
        Addressing::Implied => (0, "".into(), None),
        Addressing::Indirect => (2, format!("(${:02X}{:02X})", bank[1], bank[0]), None),
        Addressing::IndirectY => (1, format!("(${:02X}),Y", bank[0]), None),
        Addressing::Relative => {
            let offset = bank[0] as i8 as isize;
            let position = position as isize + offset + 2;
            (1, format!("L{:06X}", position), Some(position as usize))
        }
        Addressing::XIndirect => (1, format!("(${:02X},X)", bank[0]), None),
        Addressing::ZeroPage => (1, format!("${:02X}", bank[0]), None),
        Addressing::ZeroPageX => (1, format!("${:02X},X", bank[0]), None),
        Addressing::ZeroPageY => (1, format!("${:02X},Y", bank[0]), None),
    })
}

fn get_target(id: u8, lo: u8, hi: u8, rom_data: RomData) -> (String, usize) {
    let addr = ((hi as usize) << 8) + (lo as usize);

    // check if RAM address
    if addr < 0x0800 || (addr >= 0x6000 && addr < 0x8000) {
        return (format!("${addr:04X}"), addr);
    }

    // MMC4 = last bank is fixed at $C000-FFFF
    let target = if addr >= 0xC000 {
        ((rom_data.banks_count - 1) as usize) << 16
    } else {
        (id as usize) << 16
    } + addr;

    (format!("L{target:06X}.w"), target)
}

enum Addressing {
    Absolute,
    AbsoluteX,
    AbsoluteY,
    Accumulator,
    Immediate,
    Implied,
    Indirect,
    IndirectY,
    Relative,
    XIndirect,
    ZeroPage,
    ZeroPageX,
    ZeroPageY,
}

struct Opcode {
    name: &'static str,
    addressing: Addressing,
}

const OPCODES: [Option<Opcode>; 256] = [
    Some(Opcode {
        name: "BRK",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "ORA",
        addressing: Addressing::XIndirect,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "ORA",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "ASL",
        addressing: Addressing::ZeroPage,
    }),
    None,
    Some(Opcode {
        name: "PHP",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "ORA",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "ASL",
        addressing: Addressing::Accumulator,
    }),
    None,
    None,
    Some(Opcode {
        name: "ORA",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "ASL",
        addressing: Addressing::Absolute,
    }),
    None,
    Some(Opcode {
        name: "BPL",
        addressing: Addressing::Relative,
    }),
    Some(Opcode {
        name: "ORA",
        addressing: Addressing::IndirectY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "ORA",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "ASL",
        addressing: Addressing::ZeroPageX,
    }),
    None,
    Some(Opcode {
        name: "CLC",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "ORA",
        addressing: Addressing::AbsoluteY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "ORA",
        addressing: Addressing::AbsoluteX,
    }),
    Some(Opcode {
        name: "ASL",
        addressing: Addressing::AbsoluteX,
    }),
    None,
    Some(Opcode {
        name: "JSR",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "AND",
        addressing: Addressing::XIndirect,
    }),
    None,
    None,
    Some(Opcode {
        name: "BIT",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "AND",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "ROL",
        addressing: Addressing::ZeroPage,
    }),
    None,
    Some(Opcode {
        name: "PLP",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "AND",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "ROL",
        addressing: Addressing::Accumulator,
    }),
    None,
    Some(Opcode {
        name: "BIT",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "AND",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "ROL",
        addressing: Addressing::Absolute,
    }),
    None,
    Some(Opcode {
        name: "BMI",
        addressing: Addressing::Relative,
    }),
    Some(Opcode {
        name: "AND",
        addressing: Addressing::IndirectY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "AND",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "ROL",
        addressing: Addressing::ZeroPageX,
    }),
    None,
    Some(Opcode {
        name: "SEC",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "AND",
        addressing: Addressing::AbsoluteY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "AND",
        addressing: Addressing::AbsoluteX,
    }),
    Some(Opcode {
        name: "ROL",
        addressing: Addressing::AbsoluteX,
    }),
    None,
    Some(Opcode {
        name: "RTI",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "EOR",
        addressing: Addressing::XIndirect,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "EOR",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "LSR",
        addressing: Addressing::ZeroPage,
    }),
    None,
    Some(Opcode {
        name: "PHA",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "EOR",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "LSR",
        addressing: Addressing::Accumulator,
    }),
    None,
    Some(Opcode {
        name: "JMP",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "EOR",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "LSR",
        addressing: Addressing::Absolute,
    }),
    None,
    Some(Opcode {
        name: "BVC",
        addressing: Addressing::Relative,
    }),
    Some(Opcode {
        name: "EOR",
        addressing: Addressing::IndirectY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "EOR",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "LSR",
        addressing: Addressing::ZeroPageX,
    }),
    None,
    Some(Opcode {
        name: "CLI",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "EOR",
        addressing: Addressing::AbsoluteY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "EOR",
        addressing: Addressing::AbsoluteX,
    }),
    Some(Opcode {
        name: "LSR",
        addressing: Addressing::AbsoluteX,
    }),
    None,
    Some(Opcode {
        name: "RTS",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "ADC",
        addressing: Addressing::XIndirect,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "ADC",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "ROR",
        addressing: Addressing::ZeroPage,
    }),
    None,
    Some(Opcode {
        name: "PLA",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "ADC",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "ROR",
        addressing: Addressing::Accumulator,
    }),
    None,
    Some(Opcode {
        name: "JMP",
        addressing: Addressing::Indirect,
    }),
    Some(Opcode {
        name: "ADC",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "ROR",
        addressing: Addressing::Absolute,
    }),
    None,
    Some(Opcode {
        name: "BVS",
        addressing: Addressing::Relative,
    }),
    Some(Opcode {
        name: "ADC",
        addressing: Addressing::IndirectY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "ADC",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "ROR",
        addressing: Addressing::ZeroPageX,
    }),
    None,
    Some(Opcode {
        name: "SEI",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "ADC",
        addressing: Addressing::AbsoluteY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "ADC",
        addressing: Addressing::AbsoluteX,
    }),
    Some(Opcode {
        name: "ROR",
        addressing: Addressing::AbsoluteX,
    }),
    None,
    None,
    Some(Opcode {
        name: "STA",
        addressing: Addressing::XIndirect,
    }),
    None,
    None,
    Some(Opcode {
        name: "STY",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "STA",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "STX",
        addressing: Addressing::ZeroPage,
    }),
    None,
    Some(Opcode {
        name: "DEY",
        addressing: Addressing::Implied,
    }),
    None,
    Some(Opcode {
        name: "TXA",
        addressing: Addressing::Implied,
    }),
    None,
    Some(Opcode {
        name: "STY",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "STA",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "STX",
        addressing: Addressing::Absolute,
    }),
    None,
    Some(Opcode {
        name: "BCC",
        addressing: Addressing::Relative,
    }),
    Some(Opcode {
        name: "STA",
        addressing: Addressing::IndirectY,
    }),
    None,
    None,
    Some(Opcode {
        name: "STY",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "STA",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "STX",
        addressing: Addressing::ZeroPageY,
    }),
    None,
    Some(Opcode {
        name: "TYA",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "STA",
        addressing: Addressing::AbsoluteY,
    }),
    Some(Opcode {
        name: "TXS",
        addressing: Addressing::Implied,
    }),
    None,
    None,
    Some(Opcode {
        name: "STA",
        addressing: Addressing::AbsoluteX,
    }),
    None,
    None,
    Some(Opcode {
        name: "LDY",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "LDA",
        addressing: Addressing::XIndirect,
    }),
    Some(Opcode {
        name: "LDX",
        addressing: Addressing::Immediate,
    }),
    None,
    Some(Opcode {
        name: "LDY",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "LDA",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "LDX",
        addressing: Addressing::ZeroPage,
    }),
    None,
    Some(Opcode {
        name: "TAY",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "LDA",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "TAX",
        addressing: Addressing::Implied,
    }),
    None,
    Some(Opcode {
        name: "LDY",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "LDA",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "LDX",
        addressing: Addressing::Absolute,
    }),
    None,
    Some(Opcode {
        name: "BCS",
        addressing: Addressing::Relative,
    }),
    Some(Opcode {
        name: "LDA",
        addressing: Addressing::IndirectY,
    }),
    None,
    None,
    Some(Opcode {
        name: "LDY",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "LDA",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "LDX",
        addressing: Addressing::ZeroPageY,
    }),
    None,
    Some(Opcode {
        name: "CLV",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "LDA",
        addressing: Addressing::AbsoluteY,
    }),
    Some(Opcode {
        name: "TSX",
        addressing: Addressing::Implied,
    }),
    None,
    Some(Opcode {
        name: "LDY",
        addressing: Addressing::AbsoluteX,
    }),
    Some(Opcode {
        name: "LDA",
        addressing: Addressing::AbsoluteX,
    }),
    Some(Opcode {
        name: "LDX",
        addressing: Addressing::AbsoluteY,
    }),
    None,
    Some(Opcode {
        name: "CPY",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "CMP",
        addressing: Addressing::XIndirect,
    }),
    None,
    None,
    Some(Opcode {
        name: "CPY",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "CMP",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "DEC",
        addressing: Addressing::ZeroPage,
    }),
    None,
    Some(Opcode {
        name: "INY",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "CMP",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "DEX",
        addressing: Addressing::Implied,
    }),
    None,
    Some(Opcode {
        name: "CPY",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "CMP",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "DEC",
        addressing: Addressing::Absolute,
    }),
    None,
    Some(Opcode {
        name: "BNE",
        addressing: Addressing::Relative,
    }),
    Some(Opcode {
        name: "CMP",
        addressing: Addressing::IndirectY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "CMP",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "DEC",
        addressing: Addressing::ZeroPageX,
    }),
    None,
    Some(Opcode {
        name: "CLD",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "CMP",
        addressing: Addressing::AbsoluteY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "CMP",
        addressing: Addressing::AbsoluteX,
    }),
    Some(Opcode {
        name: "DEC",
        addressing: Addressing::AbsoluteX,
    }),
    None,
    Some(Opcode {
        name: "CPX",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "SBC",
        addressing: Addressing::XIndirect,
    }),
    None,
    None,
    Some(Opcode {
        name: "CPX",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "SBC",
        addressing: Addressing::ZeroPage,
    }),
    Some(Opcode {
        name: "INC",
        addressing: Addressing::ZeroPage,
    }),
    None,
    Some(Opcode {
        name: "INX",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "SBC",
        addressing: Addressing::Immediate,
    }),
    Some(Opcode {
        name: "NOP",
        addressing: Addressing::Implied,
    }),
    None,
    Some(Opcode {
        name: "CPX",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "SBC",
        addressing: Addressing::Absolute,
    }),
    Some(Opcode {
        name: "INC",
        addressing: Addressing::Absolute,
    }),
    None,
    Some(Opcode {
        name: "BEQ",
        addressing: Addressing::Relative,
    }),
    Some(Opcode {
        name: "SBC",
        addressing: Addressing::IndirectY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "SBC",
        addressing: Addressing::ZeroPageX,
    }),
    Some(Opcode {
        name: "INC",
        addressing: Addressing::ZeroPageX,
    }),
    None,
    Some(Opcode {
        name: "SED",
        addressing: Addressing::Implied,
    }),
    Some(Opcode {
        name: "SBC",
        addressing: Addressing::AbsoluteY,
    }),
    None,
    None,
    None,
    Some(Opcode {
        name: "SBC",
        addressing: Addressing::AbsoluteX,
    }),
    Some(Opcode {
        name: "INC",
        addressing: Addressing::AbsoluteX,
    }),
    None,
];

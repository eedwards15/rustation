pub mod bios;
mod ram;
mod dma;

use self::bios::Bios;
use self::ram::Ram;
use self::dma::{Dma, Port, Direction, Step, Sync};
use gpu::Gpu;

/// Global interconnect
pub struct Interconnect {
    /// Basic Input/Output memory
    bios: Bios,
    /// Main RAM
    ram: Ram,
    /// DMA registers
    dma: Dma,
    /// Graphics Processor Unit
    gpu: Gpu,
    /// Cache Control register
    cache_control: CacheControl,
}

impl Interconnect {
    pub fn new(bios: Bios, gpu: Gpu) -> Interconnect {
        Interconnect {
            bios: bios,
            ram: Ram::new(),
            dma: Dma::new(),
            gpu: gpu,
            cache_control: CacheControl(0),
        }
    }

    pub fn cache_control(&self) -> CacheControl {
        self.cache_control
    }

    /// Interconnect: load value at `addr`
    pub fn load<T: Addressable>(&self, addr: u32) -> T {
        let abs_addr = map::mask_region(addr);

        if let Some(offset) = map::RAM.contains(abs_addr) {
            return self.ram.load(offset);
        }

        if let Some(offset) = map::BIOS.contains(abs_addr) {
            return self.bios.load(offset);
        }

        if let Some(offset) = map::IRQ_CONTROL.contains(abs_addr) {
            println!("IRQ control read {:x}", offset);
            return Addressable::from_u32(0);
        }

        if let Some(offset) = map::DMA.contains(abs_addr) {
            return self.dma_reg(offset);
        }

        if let Some(offset) = map::GPU.contains(abs_addr) {
            return self.gpu.load(offset);
        }

        if let Some(offset) = map::TIMERS.contains(abs_addr) {
            println!("Unhandled read from timer register {:x}",
                     offset);
            return Addressable::from_u32(0);
        }

        if let Some(_) = map::SPU.contains(abs_addr) {
            println!("Unhandled read from SPU register {:08x}", abs_addr);
            return Addressable::from_u32(0);
        }

        if let Some(_) = map::EXPANSION_1.contains(abs_addr) {
            // No expansion implemented. Returns full ones when no
            // expansion is present
            return Addressable::from_u32(!0);
        }

        panic!("unhandled load at address {:08x}", addr);
    }

    /// Interconnect: store `val` into `addr`
    pub fn store<T: Addressable>(&mut self, addr: u32, val: T) {

        let abs_addr = map::mask_region(addr);

        if let Some(offset) = map::RAM.contains(abs_addr) {
            return self.ram.store(offset, val);
        }

        if let Some(offset) = map::IRQ_CONTROL.contains(abs_addr) {
            println!("IRQ control: {:x} <- {:08x}", offset, val.as_u32());
            return;
        }

        if let Some(offset) = map::DMA.contains(abs_addr) {
            return self.set_dma_reg(offset, val);
        }

        if let Some(offset) = map::GPU.contains(abs_addr) {
            return self.gpu.store(offset, val);
        }

        if let Some(offset) = map::TIMERS.contains(abs_addr) {
            println!("Unhandled write to timer register {:x}: {:08x}",
                     offset, val.as_u32());
            return;
        }

        if let Some(_) = map::SPU.contains(abs_addr) {
            println!("Unhandled write to SPU register {:08x}: {:04x}",
                     abs_addr, val.as_u32());
            return;
        }

        if let Some(_) = map::CACHE_CONTROL.contains(abs_addr) {
            if T::width() != AccessWidth::Word {
                panic!("Unhandled cache control access");
            }

            self.cache_control = CacheControl(val.as_u32());

            return;
        }

        if let Some(offset) = map::MEM_CONTROL.contains(abs_addr) {
            let val = val.as_u32();

            match offset {
                0 => // Expansion 1 base address
                    if val != 0x1f000000 {
                        panic!("Bad expansion 1 base address: 0x{:08x}", val);
                    },
                4 => // Expansion 2 base address
                    if val != 0x1f802000 {
                        panic!("Bad expansion 2 base address: 0x{:08x}", val);
                    },
                _ =>
                    println!("Unhandled write to MEM_CONTROL register {:x}: \
                              0x{:08x}",
                             offset, val),
            }

            return;
        }

        if let Some(_) = map::RAM_SIZE.contains(abs_addr) {
            // We ignore writes at this address
            return;
        }

        if let Some(offset) = map::EXPANSION_2.contains(abs_addr) {
            println!("Unhandled write to expansion 2 register {:x}", offset);
            return;
        }

        panic!("unhandled store32 into address {:08x}: {:08x}",
               addr, val.as_u32());
    }

    /// DMA register read
    fn dma_reg<T: Addressable>(&self, offset: u32) -> T {

        if T::width() != AccessWidth::Word {
            panic!("Unhandled {:?} DMA load", T::width());
        }

        let major = (offset & 0x70) >> 4;
        let minor = offset & 0xf;

        let res =
            match major {
                // Per-channel registers
                0...6 => {
                    let channel = self.dma.channel(Port::from_index(major));

                    match minor {
                        0 => channel.base(),
                        4 => channel.block_control(),
                        8 => channel.control(),
                        _ => panic!("Unhandled DMA read at {:x}", offset)
                    }
                },
                // Common DMA registers
                7 => match minor {
                    0 => self.dma.control(),
                    4 => self.dma.interrupt(),
                    _ => panic!("Unhandled DMA read at {:x}", offset)
                },
                _ => panic!("Unhandled DMA read at {:x}", offset)
            };

        Addressable::from_u32(res)
    }

    /// DMA register write
    fn set_dma_reg<T: Addressable>(&mut self, offset: u32, val: T) {
        if T::width() != AccessWidth::Word {
            panic!("Unhandled {:?} DMA store", T::width());
        }

        let val = val.as_u32();

        let major = (offset & 0x70) >> 4;
        let minor = offset & 0xf;

        let active_port =
            match major {
                // Per-channel registers
                0...6 => {
                    let port = Port::from_index(major);
                    let channel = self.dma.channel_mut(port);

                    match minor {
                        0 => channel.set_base(val),
                        4 => channel.set_block_control(val),
                        8 => channel.set_control(val),
                        _ => panic!("Unhandled DMA write {:x}: {:08x}",
                                    offset, val)
                    }

                    if channel.active() {
                        Some(port)
                    } else {
                        None
                    }
                },
                // Common DMA registers
                7 => {
                    match minor {
                        0 => self.dma.set_control(val),
                        4 => self.dma.set_interrupt(val),
                        _ => panic!("Unhandled DMA write {:x}: {:08x}",
                                    offset, val),
                    }

                    None
                }
                _ => panic!("Unhandled DMA write {:x}: {:08x}",
                            offset, val),
            };

        if let Some(port) = active_port {
            self.do_dma(port);
        }
    }

    /// Execute DMA transfer for a port
    fn do_dma(&mut self, port: Port) {
        // DMA transfer has been started, for now let's
        // process everything in one pass (i.e. no
        // chopping or priority handling)

        match self.dma.channel(port).sync() {
                Sync::LinkedList => self.do_dma_linked_list(port),
                _                => self.do_dma_block(port),
        }
    }

    /// Emulate DMA transfer for linked list synchronization mode.
    fn do_dma_linked_list(&mut self, port: Port) {
        let channel = self.dma.channel_mut(port);

        let mut addr = channel.base() & 0x1ffffc;

        if channel.direction() == Direction::ToRam {
            panic!("Invalid DMA direction for linked list mode");
        }

        // I don't know if the DMA even supports linked list mode for
        // anything besides the GPU
        if port != Port::Gpu {
            panic!("Attempted linked list DMA on port {}", port as u8);
        }

        loop {
            // In linked list mode, each entry starts with a "header"
            // word. The high byte contains the number of words in the
            // "packet" (not counting the header word)
            let header = self.ram.load::<u32>(addr);

            let mut remsz = header >> 24;

            while remsz > 0 {
                addr = (addr + 4) & 0x1ffffc;

                let command = self.ram.load::<u32>(addr);

                // Send command to the GPU
                self.gpu.gp0(command);

                remsz -= 1;
            }

            // The end-of-table marker is usually 0xffffff but
            // mednafen only checks for the MSB so maybe that's what
            // the hardware does? Since this bit is not part of any
            // valid address it makes some sense. I'll have to test
            // that at some point...
            if header & 0x800000 != 0 {
                break;
            }

            addr = header & 0x1ffffc;
        }

        channel.done();
    }

    /// Emulate DMA transfer for Manual and Request synchronization
    /// modes.
    fn do_dma_block(&mut self, port: Port) {
        let channel = self.dma.channel_mut(port);

        let increment = match channel.step() {
            Step::Increment =>  4,
            Step::Decrement => -4,
        };

        let mut addr = channel.base();

        // Transfer size in words
        let mut remsz = match channel.transfer_size() {
            Some(n) => n,
            // Shouldn't happen since we shouldn't be reaching this code
            // in linked list mode
            None    => panic!("Couldn't figure out DMA block transfer size"),
        };

        while remsz > 0 {
            // Not sure what happens if address is
            // bogus... Mednafen just masks addr this way, maybe
            // that's how the hardware behaves (i.e. the RAM
            // address wraps and the two LSB are ignored, seems
            // reasonable enough
            let cur_addr = addr & 0x1ffffc;

            match channel.direction() {
                Direction::FromRam => {
                    let src_word = self.ram.load::<u32>(cur_addr);

                    match port {
                        Port::Gpu => self.gpu.gp0(src_word),
                        _ => panic!("Unhandled DMA destination port {}",
                                    port as u8),
                    }
                }
                Direction::ToRam => {
                    let src_word = match port {
                        // Clear ordering table
                        Port::Otc => match remsz {
                            // Last entry contains the end
                            // of table marker
                            1 => 0xffffff,
                            // Pointer to the previous entry
                            _ => addr.wrapping_sub(4) & 0x1fffff,
                        },
                        _ => panic!("Unhandled DMA source port {}", port as u8),
                    };

                    self.ram.store(cur_addr, src_word);
                }
            }

            addr = addr.wrapping_add(increment);
            remsz -= 1;
        }

        channel.done();
    }
}

#[derive(Clone,Copy)]
pub struct CacheControl(u32);

impl CacheControl {

    /// Return true if the instruction cache is enabled
    pub fn icache_enabled(self) -> bool {
        self.0 & 0x800 != 0
    }

    pub fn tag_test_mode(self) -> bool {
        self.0 & 4 != 0
    }
}

/// Types of access supported by the Playstation architecture
#[derive(PartialEq,Eq,Debug)]
pub enum AccessWidth {
    Byte = 1,
    Halfword = 2,
    Word = 4,
}

/// rait representing the attributes of a primitive addressable
/// memory location.
pub trait Addressable {
    /// Retreive the width of the access
    fn width() -> AccessWidth;
    /// Build an Addressable value from an u32. If the Addressable is 8
    /// or 16bits wide the MSBs are discarded to fit.
    fn from_u32(u32) -> Self;
    /// Retreive the value of the Addressable as an u32. If the
    /// Addressable is 8 or 16bits wide the MSBs are padded with 0s.
    fn as_u32(&self) -> u32;
}

impl Addressable for u8 {
    fn width() -> AccessWidth {
        AccessWidth::Byte
    }

    fn from_u32(v: u32) -> u8 {
        v as u8
    }

    fn as_u32(&self) -> u32 {
        *self as u32
    }
}

impl Addressable for u16 {
    fn width() -> AccessWidth {
        AccessWidth::Halfword
    }

    fn from_u32(v: u32) -> u16 {
        v as u16
    }

    fn as_u32(&self) -> u32 {
        *self as u32
    }
}

impl Addressable for u32 {
    fn width() -> AccessWidth {
        AccessWidth::Word
    }

    fn from_u32(v: u32) -> u32 {
        v
    }

    fn as_u32(&self) -> u32 {
        *self
    }
}

mod map {
    pub struct Range(u32, u32);

    impl Range {
        /// Return `Some(offset)` if addr is contained in `self`
        pub fn contains(self, addr: u32) -> Option<u32> {
            let Range(start, length) = self;

            if addr >= start && addr < start + length {
                Some(addr - start)
            } else {
                None
            }
        }
    }

    /// Mask array used to strip the region bits of the address. The
    /// mask is selected using the 3 MSBs of the address so each entry
    /// effectively matches 512kB of the address space. KSEG2 is not
    /// touched since it doesn't share anything with the other
    /// regions.
    const REGION_MASK: [u32; 8] = [
        0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, // KUSEG: 2048MB
        0x7fffffff,                                     // KSEG0:  512MB
        0x1fffffff,                                     // KSEG1:  512MB
        0xffffffff, 0xffffffff,                         // KSEG2: 1024MB
        ];

    /// Mask a CPU address to remove the region bits.
    pub fn mask_region(addr: u32) -> u32 {
        // Index address space in 512MB chunks
        let index = (addr >> 29) as usize;

        addr & REGION_MASK[index]
    }

    pub const RAM: Range = Range(0x00000000, 2 * 1024 * 1024);

    /// Expansion region 1
    pub const EXPANSION_1: Range = Range(0x1f000000, 512 * 1024);

    pub const BIOS: Range = Range(0x1fc00000, 512 * 1024);

    /// Memory latency and expansion mapping
    pub const MEM_CONTROL: Range = Range(0x1f801000, 36);

    /// Register that has something to do with RAM configuration,
    /// configured by the BIOS
    pub const RAM_SIZE: Range = Range(0x1f801060, 4);

    /// Interrupt Control registers (status and mask)
    pub const IRQ_CONTROL: Range = Range(0x1f801070, 8);

    /// Direct Memory Access registers
    pub const DMA: Range = Range(0x1f801080, 0x80);

    pub const TIMERS: Range = Range(0x1f801100, 0x30);

    pub const GPU: Range = Range(0x1f801810, 8);

    /// SPU registers
    pub const SPU: Range = Range(0x1f801c00, 640);

    /// Expansion region 2
    pub const EXPANSION_2: Range = Range(0x1f802000, 66);

    /// Cache control register. Full address since it's in KSEG2
    pub const CACHE_CONTROL: Range = Range(0xfffe0130, 4);
}

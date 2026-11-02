//! SIO0: controllers (digital / DualShock) and memory cards.

use serde::{Deserialize, Serialize};

use std::collections::VecDeque;

pub const MEMCARD_SIZE: usize = 128 * 1024;
const ACK_DELAY: i32 = 450;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PadType {
    None,
    Digital,
    DualShock,
}

#[derive(Serialize, Deserialize)]
pub struct Pad {
    pub kind: PadType,
    /// Bit set = pressed (converted to active-low on the wire).
    pub buttons: u16,
    /// rx, ry, lx, ly (0x80 = centre)
    pub axes: [u8; 4],
    pub analog_mode: bool,
    analog_locked: bool,
    config_mode: bool,
    rumble_map: [u8; 6],
    pub rumble: (u8, u8), // small, large
    cmd: u8,
    reply: Vec<u8>,
    rx: Vec<u8>,
}

impl Pad {
    fn new(kind: PadType) -> Self {
        Pad {
            kind,
            buttons: 0,
            axes: [0x80; 4],
            analog_mode: false,
            analog_locked: false,
            config_mode: false,
            rumble_map: [0xFF; 6],
            rumble: (0, 0),
            cmd: 0,
            reply: Vec::new(),
            rx: Vec::new(),
        }
    }

    fn id(&self) -> u8 {
        if self.config_mode {
            0xF3
        } else if self.kind == PadType::DualShock && self.analog_mode {
            0x73
        } else {
            0x41
        }
    }

    fn poll_bytes(&self) -> Vec<u8> {
        let b = !self.buttons;
        let mut v = vec![b as u8, (b >> 8) as u8];
        if self.id() == 0x73 || self.config_mode {
            v.extend_from_slice(&self.axes);
        }
        v
    }

    /// Byte exchange. `step` is the byte index after the address byte (0-based).
    fn transfer(&mut self, step: usize, tx: u8) -> (u8, bool) {
        if step == 0 {
            self.cmd = tx;
            self.rx.clear();
            let ds = self.kind == PadType::DualShock;
            self.reply = match tx {
                0x42 => {
                    let mut r = vec![0x5A];
                    r.extend(self.poll_bytes());
                    r
                }
                0x43 if ds => {
                    let mut r = vec![0x5A];
                    if self.config_mode {
                        r.extend_from_slice(&[0; 6]);
                    } else {
                        r.extend(self.poll_bytes());
                    }
                    r
                }
                0x44 | 0x46 | 0x47 | 0x4C | 0x4D | 0x45 if ds && self.config_mode => {
                    let mut r = vec![0x5A];
                    r.extend_from_slice(&match tx {
                        0x45 => [0x01, 0x02, self.analog_mode as u8, 0x02, 0x01, 0x00],
                        0x47 => [0x00, 0x00, 0x02, 0x00, 0x01, 0x00],
                        0x4D => self.rumble_map,
                        _ => [0; 6],
                    });
                    r
                }
                _ => {
                    // unsupported command: no reply/ack after the ID byte
                    return (0xFF, false);
                }
            };
            let id = self.id();
            return (id, true);
        }
        self.rx.push(tx);
        let k = step - 1;
        // command-specific parameter handling
        match self.cmd {
            0x42 if self.id() == 0x73 => {
                let a = self.rumble_map;
                for (i, &m) in a.iter().enumerate() {
                    if i == k.wrapping_sub(1) {
                        if m == 0x00 {
                            self.rumble.0 = if tx & 1 != 0 { 0xFF } else { 0 };
                        } else if m == 0x01 {
                            self.rumble.1 = tx;
                        }
                    }
                }
            }
            0x43 if k == 1 => {
                self.config_mode = tx == 1;
            }
            0x44 => {
                if k == 1 {
                    self.analog_mode = tx == 1;
                }
                if k == 2 {
                    self.analog_locked = tx == 3;
                }
            }
            0x46 if k == 1 => {
                self.reply = vec![0x5A, 0, 0, 1, if tx == 1 { 1 } else { 2 }, if tx == 1 { 1 } else { 0 }, if tx == 1 { 0x14 } else { 0x0A }];
            }
            0x4C if k == 1 => {
                self.reply = vec![0x5A, 0, 0, 0, if tx == 1 { 0x07 } else { 0x04 }, 0, 0];
            }
            0x4D if (1..=6).contains(&k) => {
                self.rumble_map[k - 1] = tx;
            }
            _ => {}
        }
        let r = self.reply.get(k).copied().unwrap_or(0xFF);
        let more = k + 1 < self.reply.len();
        (r, more)
    }
}

#[derive(Serialize, Deserialize)]
pub struct MemCard {
    #[serde(skip)]
    pub data: Vec<u8>,
    pub present: bool,
    pub dirty: bool,
    flag: u8,
    cmd: u8,
    msb: u8,
    lsb: u8,
    chk: u8,
    last: u8,
}

impl MemCard {
    fn new() -> Self {
        let mut c = MemCard {
            data: vec![0; MEMCARD_SIZE],
            present: true,
            dirty: false,
            flag: 0x08,
            cmd: 0,
            msb: 0,
            lsb: 0,
            chk: 0,
            last: 0,
        };
        c.format();
        c
    }

    /// Write an empty, formatted card image.
    pub fn format(&mut self) {
        self.data.fill(0);
        // header frame
        self.data[0] = b'M';
        self.data[1] = b'C';
        self.data[127] = b'M' ^ b'C';
        // directory frames 1..15
        for i in 1..16 {
            let o = i * 128;
            self.data[o] = 0xA0;
            self.data[o + 8] = 0xFF;
            self.data[o + 9] = 0xFF;
            let mut x = 0u8;
            for k in 0..127 {
                x ^= self.data[o + k];
            }
            self.data[o + 127] = x;
        }
        // broken sector list frames 16..35
        for i in 16..36 {
            let o = i * 128;
            self.data[o..o + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
            self.data[o + 8] = 0xFF;
            self.data[o + 9] = 0xFF;
            let mut x = 0u8;
            for k in 0..127 {
                x ^= self.data[o + k];
            }
            self.data[o + 127] = x;
        }
        // frame 63 test write area mirrors header
        let hdr: Vec<u8> = self.data[0..128].to_vec();
        self.data[63 * 128..64 * 128].copy_from_slice(&hdr);
        self.dirty = true;
    }

    fn transfer(&mut self, step: usize, tx: u8) -> (u8, bool) {
        let prev = self.last;
        self.last = tx;
        if step == 0 {
            self.cmd = tx;
            return match tx {
                0x52 | 0x57 | 0x53 => (self.flag, true),
                _ => (0xFF, false),
            };
        }
        let k = step - 1;
        match self.cmd {
            0x52 => match k {
                0 => (0x5A, true),
                1 => (0x5D, true),
                2 => {
                    self.msb = tx;
                    (0x00, true)
                }
                3 => {
                    self.lsb = tx;
                    (prev, true)
                }
                4 => (0x5C, true),
                5 => (0x5D, true),
                6 => {
                    self.chk = self.msb ^ self.lsb;
                    (self.msb, true)
                }
                7 => (self.lsb, true),
                8..=135 => {
                    let sector = (((self.msb as usize) << 8) | self.lsb as usize) & 0x3FF;
                    let b = self.data[sector * 128 + (k - 8)];
                    self.chk ^= b;
                    (b, true)
                }
                136 => (self.chk, true),
                _ => (0x47, false),
            },
            0x57 => match k {
                0 => (0x5A, true),
                1 => (0x5D, true),
                2 => {
                    self.msb = tx;
                    self.chk = tx;
                    (0x00, true)
                }
                3 => {
                    self.lsb = tx;
                    self.chk ^= tx;
                    (prev, true)
                }
                4..=131 => {
                    let sector = (((self.msb as usize) << 8) | self.lsb as usize) & 0x3FF;
                    self.data[sector * 128 + (k - 4)] = tx;
                    self.chk ^= tx;
                    (prev, true)
                }
                132 => {
                    self.chk ^= tx; // becomes 0 when checksum matches
                    (prev, true)
                }
                133 => (0x5C, true),
                134 => (0x5D, true),
                _ => {
                    self.flag = 0x00;
                    self.dirty = true;
                    (if self.chk == 0 { 0x47 } else { 0x4E }, false)
                }
            },
            0x53 => {
                let r = [0x5A, 0x5D, 0x5C, 0x5D, 0x04, 0x00, 0x00, 0x80];
                (r.get(k).copied().unwrap_or(0xFF), k + 1 < r.len())
            }
            _ => (0xFF, false),
        }
    }
}

#[derive(PartialEq, Clone, Copy, Serialize, Deserialize)]
enum Dev {
    Idle,
    Pad,
    Card,
    Ignore,
}

#[derive(Serialize, Deserialize)]
pub struct Sio0 {
    pub pads: [Pad; 2],
    pub cards: [MemCard; 2],
    rx: VecDeque<u8>,
    stat_irq: bool,
    mode: u16,
    ctrl: u16,
    baud: u16,
    dev: Dev,
    step: usize,
    ack_timer: i32,
    ack_line: bool,
}

impl Sio0 {
    pub fn new() -> Self {
        Sio0 {
            pads: [Pad::new(PadType::DualShock), Pad::new(PadType::Digital)],
            cards: [MemCard::new(), MemCard::new()],
            rx: VecDeque::new(),
            stat_irq: false,
            mode: 0,
            ctrl: 0,
            baud: 0,
            dev: Dev::Idle,
            step: 0,
            ack_timer: 0,
            ack_line: false,
        }
    }

    pub fn reset(&mut self) {
        self.rx.clear();
        self.stat_irq = false;
        self.mode = 0;
        self.ctrl = 0;
        self.baud = 0;
        self.dev = Dev::Idle;
        self.step = 0;
        self.ack_timer = 0;
        for p in self.pads.iter_mut() {
            p.config_mode = false;
            if !p.analog_locked {
                p.analog_mode = false;
            }
        }
    }

    fn port(&self) -> usize {
        ((self.ctrl >> 13) & 1) as usize
    }

    pub fn read(&mut self, off: u32, size: u32) -> u32 {
        match off {
            0x0 => {
                let b = self.rx.pop_front().unwrap_or(0xFF) as u32;
                if size == 1 { b } else { b | 0xFFFF_FF00 }
            }
            0x4 => {
                let mut s = 0x5u32; // TX ready 1/2
                if !self.rx.is_empty() {
                    s |= 1 << 1;
                }
                if self.ack_line {
                    s |= 1 << 7;
                }
                if self.stat_irq {
                    s |= 1 << 9;
                }
                s
            }
            0x8 => self.mode as u32,
            0xA => self.ctrl as u32,
            0xE => self.baud as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, _size: u32, v: u32) {
        match off {
            0x0 => self.tx(v as u8),
            0x8 => self.mode = v as u16,
            0xA => {
                let v = v as u16;
                if v & 0x40 != 0 {
                    self.reset_regs();
                    return;
                }
                if v & 0x10 != 0 {
                    self.stat_irq = false;
                }
                self.ctrl = v & !0x50;
                if v & 0x2 == 0 {
                    self.dev = Dev::Idle;
                    self.step = 0;
                }
            }
            0xE => self.baud = v as u16,
            _ => {}
        }
    }

    fn reset_regs(&mut self) {
        self.ctrl = 0;
        self.mode = 0;
        self.stat_irq = false;
        self.rx.clear();
        self.dev = Dev::Idle;
        self.step = 0;
    }

    fn tx(&mut self, b: u8) {
        if self.ctrl & 0x2 == 0 {
            self.rx.push_back(0xFF);
            return;
        }
        let port = self.port();
        let (reply, ack) = match self.dev {
            Dev::Idle => {
                let has_pad = self.pads[port].kind != PadType::None;
                let has_card = self.cards[port].present;
                if b == 0x01 && has_pad {
                    self.dev = Dev::Pad;
                    self.step = 0;
                    (0xFF, true)
                } else if b == 0x81 && has_card {
                    self.dev = Dev::Card;
                    self.step = 0;
                    (0xFF, true)
                } else {
                    self.dev = Dev::Ignore;
                    (0xFF, false)
                }
            }
            Dev::Pad => {
                let r = self.pads[port].transfer(self.step, b);
                self.step += 1;
                r
            }
            Dev::Card => {
                let r = self.cards[port].transfer(self.step, b);
                self.step += 1;
                r
            }
            Dev::Ignore => (0xFF, false),
        };
        if self.rx.len() >= 8 {
            self.rx.pop_front();
        }
        self.rx.push_back(reply);
        if ack {
            self.ack_timer = ACK_DELAY;
        } else {
            self.dev = Dev::Ignore;
        }
    }

    /// Returns true when an IRQ7 edge is raised.
    pub fn tick(&mut self, cyc: u32) -> bool {
        if self.ack_timer > 0 {
            self.ack_timer -= cyc as i32;
            if self.ack_timer <= 0 {
                self.ack_timer = 0;
                self.ack_line = false;
                if self.ctrl & 0x1000 != 0 && !self.stat_irq {
                    self.stat_irq = true;
                    return true;
                }
            } else {
                self.ack_line = true;
            }
        }
        false
    }
}

//! Root counters (timers 0-2).

#[derive(Clone, Copy, Default)]
struct Timer {
    counter: u32,
    mode: u32,
    target: u32,
    frac: u32,
    released: bool, // sync mode 3: blank seen
    fired: bool,    // one-shot irq already delivered
}

pub struct Timers {
    t: [Timer; 3],
    in_hblank: bool,
    in_vblank: bool,
    hblank_count: u32,
}

impl Timers {
    pub fn new() -> Self {
        let mut t = [Timer::default(); 3];
        for x in t.iter_mut() {
            x.mode = 1 << 10;
        }
        Timers { t, in_hblank: false, in_vblank: false, hblank_count: 0 }
    }

    pub fn read(&mut self, off: u32) -> u32 {
        let n = ((off - 0x100) >> 4) as usize;
        if n > 2 {
            return 0;
        }
        match off & 0xF {
            0 => self.t[n].counter,
            4 => {
                let m = self.t[n].mode;
                self.t[n].mode &= !((1 << 11) | (1 << 12));
                m
            }
            8 => self.t[n].target,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u32) {
        let n = ((off - 0x100) >> 4) as usize;
        if n > 2 {
            return;
        }
        let t = &mut self.t[n];
        match off & 0xF {
            0 => t.counter = v & 0xFFFF,
            4 => {
                t.mode = (v & 0x3FF) | (1 << 10);
                t.counter = 0;
                t.frac = 0;
                t.released = false;
                t.fired = false;
            }
            8 => t.target = v & 0xFFFF,
            _ => {}
        }
    }

    pub fn blank_events(&mut self, hs: bool, he: bool, vs: bool, ve: bool) {
        if hs {
            self.in_hblank = true;
            self.hblank_count += 1;
            self.on_blank(0);
        }
        if he {
            self.in_hblank = false;
        }
        if vs {
            self.in_vblank = true;
            self.on_blank(1);
        }
        if ve {
            self.in_vblank = false;
        }
    }

    fn on_blank(&mut self, n: usize) {
        let t = &mut self.t[n];
        if t.mode & 1 == 0 {
            return;
        }
        match (t.mode >> 1) & 3 {
            1 | 2 => t.counter = 0,
            3 => t.released = true,
            _ => {}
        }
    }

    fn running(&self, n: usize) -> bool {
        let t = &self.t[n];
        if t.mode & 1 == 0 {
            return true;
        }
        let sm = (t.mode >> 1) & 3;
        if n == 2 {
            return sm == 1 || sm == 2;
        }
        let blank = if n == 0 { self.in_hblank } else { self.in_vblank };
        match sm {
            0 => !blank,
            1 => true,
            2 => blank,
            _ => t.released,
        }
    }

    /// Returns a bitmask (bit n = timer n) of raised interrupts.
    pub fn tick(&mut self, cyc: u32, dot_div: u32) -> u32 {
        let hb = std::mem::take(&mut self.hblank_count);
        let mut irq = 0;
        for n in 0..3 {
            if !self.running(n) {
                continue;
            }
            let src = (self.t[n].mode >> 8) & 3;
            let inc = match (n, src) {
                (0, 1) | (0, 3) => {
                    // dot clock: GPU clock (~11/7 CPU) / divider
                    let t = &mut self.t[n];
                    t.frac += cyc * 11 * 256 / 7;
                    let per = dot_div * 256;
                    let k = t.frac / per;
                    t.frac -= k * per;
                    k
                }
                (1, 1) | (1, 3) => hb,
                (2, 2) | (2, 3) => {
                    let t = &mut self.t[n];
                    t.frac += cyc;
                    let k = t.frac / 8;
                    t.frac &= 7;
                    k
                }
                _ => cyc,
            };
            if inc == 0 {
                continue;
            }
            if self.advance(n, inc) {
                irq |= 1 << n;
            }
        }
        irq
    }

    fn advance(&mut self, n: usize, inc: u32) -> bool {
        let t = &mut self.t[n];
        let old = t.counter;
        let mut c = old + inc;
        let mut trig = false;
        if old < t.target && c >= t.target {
            t.mode |= 1 << 11;
            if t.mode & (1 << 4) != 0 {
                trig = true;
            }
        }
        if t.mode & (1 << 3) != 0 && c > t.target {
            let period = t.target + 1;
            c = (c - period) % period;
        }
        if c > 0xFFFF {
            t.mode |= 1 << 12;
            if t.mode & (1 << 5) != 0 {
                trig = true;
            }
            c &= 0xFFFF;
        }
        t.counter = c;
        if !trig {
            return false;
        }
        let repeat = t.mode & (1 << 6) != 0;
        if !repeat && t.fired {
            return false;
        }
        t.fired = true;
        if t.mode & (1 << 7) != 0 {
            // toggle mode
            t.mode ^= 1 << 10;
            t.mode & (1 << 10) == 0
        } else {
            // pulse mode: bit 10 briefly low
            t.mode |= 1 << 10;
            true
        }
    }
}

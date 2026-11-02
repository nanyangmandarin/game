//! GPU: GP0/GP1 command processing, software rasterizer, display timing.

pub const EV_HBLANK_START: u8 = 1;
pub const EV_HBLANK_END: u8 = 2;
pub const EV_VBLANK_START: u8 = 4;
pub const EV_VBLANK_END: u8 = 8;

const VRAM_W: usize = 1024;
const VRAM_H: usize = 512;

const DITHER: [[i32; 4]; 4] = [[-4, 0, -3, 1], [2, -2, 3, -1], [-3, 1, -4, 0], [3, -1, 2, -2]];

#[derive(Clone, Copy, Default, Debug)]
struct Vtx {
    x: i32,
    y: i32,
    r: i32,
    g: i32,
    b: i32,
    u: i32,
    v: i32,
}

#[derive(Clone, Copy, Default)]
struct Prim {
    textured: bool,
    gouraud: bool,
    semi: bool,
    raw: bool,
    semi_mode: u32,
    tp_x: u32,
    tp_y: u32,
    depth: u32,
    clut_x: u32,
    clut_y: u32,
    dither: bool,
}

#[derive(Clone, Copy, Default)]
struct Xfer {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    cx: u32,
    cy: u32,
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Command,
    Load,
    PolyLine,
}

pub struct Gpu {
    pub vram: Vec<u16>,
    // draw mode (E1)
    tp_x: u32,
    tp_y: u32,
    semi_mode: u32,
    tex_depth: u32,
    dither: bool,
    draw_to_display: bool,
    tex_disable: bool,
    flip_x: bool,
    flip_y: bool,
    // E2..E6
    win_mx: u32,
    win_my: u32,
    win_ox: u32,
    win_oy: u32,
    da_x1: i32,
    da_y1: i32,
    da_x2: i32,
    da_y2: i32,
    off_x: i32,
    off_y: i32,
    set_mask: bool,
    check_mask: bool,
    // display
    pub disp_on: bool,
    dma_dir: u32,
    pub disp_x: u32,
    pub disp_y: u32,
    h_range: (u32, u32),
    v_range: (u32, u32),
    hres1: u32,
    hres2: bool,
    vres480: bool,
    pub pal: bool,
    pub depth24: bool,
    pub interlace: bool,
    irq: bool,
    irq_edge: bool,
    // command fifo
    cmd: Vec<u32>,
    need: usize,
    mode: Mode,
    load: Xfer,
    read: Xfer,
    reading: bool,
    gpuread: u32,
    // timing
    line: u32,
    line_cyc: u32,
    field: bool,
    in_hblank: bool,
    in_vblank: bool,
    // options
    pub opt_dither: bool,
    pub stat_pixels: u64,
    pub stat_cmds: u32,
}

impl Gpu {
    pub fn new() -> Self {
        let mut g = Gpu {
            vram: vec![0; VRAM_W * VRAM_H],
            tp_x: 0,
            tp_y: 0,
            semi_mode: 0,
            tex_depth: 0,
            dither: false,
            draw_to_display: false,
            tex_disable: false,
            flip_x: false,
            flip_y: false,
            win_mx: 0,
            win_my: 0,
            win_ox: 0,
            win_oy: 0,
            da_x1: 0,
            da_y1: 0,
            da_x2: 0,
            da_y2: 0,
            off_x: 0,
            off_y: 0,
            set_mask: false,
            check_mask: false,
            disp_on: false,
            dma_dir: 0,
            disp_x: 0,
            disp_y: 0,
            h_range: (0x200, 0xC00),
            v_range: (0x10, 0x100),
            hres1: 0,
            hres2: false,
            vres480: false,
            pal: false,
            depth24: false,
            interlace: false,
            irq: false,
            irq_edge: false,
            cmd: Vec::with_capacity(16),
            need: 0,
            mode: Mode::Command,
            load: Xfer::default(),
            read: Xfer::default(),
            reading: false,
            gpuread: 0,
            line: 0,
            line_cyc: 0,
            field: false,
            in_hblank: false,
            in_vblank: false,
            opt_dither: true,
            stat_pixels: 0,
            stat_cmds: 0,
        };
        g.reset_state();
        g
    }

    pub fn reset(&mut self) {
        let dither = self.opt_dither;
        *self = Gpu::new();
        self.opt_dither = dither;
    }

    fn reset_state(&mut self) {
        self.tp_x = 0;
        self.tp_y = 0;
        self.semi_mode = 0;
        self.tex_depth = 0;
        self.dither = false;
        self.draw_to_display = false;
        self.tex_disable = false;
        self.flip_x = false;
        self.flip_y = false;
        self.win_mx = 0;
        self.win_my = 0;
        self.win_ox = 0;
        self.win_oy = 0;
        self.da_x1 = 0;
        self.da_y1 = 0;
        self.da_x2 = 0;
        self.da_y2 = 0;
        self.off_x = 0;
        self.off_y = 0;
        self.set_mask = false;
        self.check_mask = false;
        self.disp_on = false;
        self.dma_dir = 0;
        self.disp_x = 0;
        self.disp_y = 0;
        self.h_range = (0x200, 0xC00);
        self.v_range = (0x10, 0x100);
        self.hres1 = 0;
        self.hres2 = false;
        self.vres480 = false;
        self.pal = false;
        self.depth24 = false;
        self.interlace = false;
        self.irq = false;
        self.cmd.clear();
        self.mode = Mode::Command;
        self.reading = false;
    }

    // ---------------- status / timing ----------------

    pub fn status(&self) -> u32 {
        let mut s = self.tp_x
            | (self.tp_y << 4)
            | (self.semi_mode << 5)
            | (self.tex_depth << 7)
            | ((self.dither as u32) << 9)
            | ((self.draw_to_display as u32) << 10)
            | ((self.set_mask as u32) << 11)
            | ((self.check_mask as u32) << 12)
            | ((self.field as u32) << 13)
            | ((self.tex_disable as u32) << 15)
            | ((self.hres2 as u32) << 16)
            | (self.hres1 << 17)
            | ((self.vres480 as u32) << 19)
            | ((self.pal as u32) << 20)
            | ((self.depth24 as u32) << 21)
            | ((self.interlace as u32) << 22)
            | ((!self.disp_on as u32) << 23)
            | ((self.irq as u32) << 24)
            | (1 << 26)
            | (1 << 27)
            | (1 << 28)
            | (self.dma_dir << 29);
        s |= match self.dma_dir {
            1 => 1 << 25,
            2 => 1 << 25,
            3 => 1 << 25,
            _ => 0,
        };
        let odd = if self.interlace && self.vres480 {
            self.field
        } else {
            !self.in_vblank && (self.line & 1) == 1
        };
        s |= (odd as u32) << 31;
        s
    }

    pub fn take_irq(&mut self) -> bool {
        std::mem::take(&mut self.irq_edge)
    }

    fn line_cycles(&self) -> u32 {
        if self.pal { 2169 } else { 2154 }
    }
    fn total_lines(&self) -> u32 {
        if self.pal { 314 } else { 263 }
    }
    fn vblank_line(&self) -> u32 {
        if self.pal { 288 } else { 240 }
    }

    pub fn dot_divider(&self) -> u32 {
        if self.hres2 {
            7
        } else {
            match self.hres1 {
                0 => 10,
                1 => 8,
                2 => 5,
                _ => 4,
            }
        }
    }

    pub fn tick(&mut self, cyc: u32) -> u8 {
        let mut ev = 0;
        self.line_cyc += cyc;
        let lc = self.line_cycles();
        let hb_start = lc * 4 / 5;
        if !self.in_hblank && self.line_cyc >= hb_start {
            self.in_hblank = true;
            ev |= EV_HBLANK_START;
        }
        if self.line_cyc >= lc {
            self.line_cyc -= lc;
            self.in_hblank = false;
            ev |= EV_HBLANK_END;
            self.line += 1;
            if self.line == self.vblank_line() {
                self.in_vblank = true;
                ev |= EV_VBLANK_START;
            }
            if self.line >= self.total_lines() {
                self.line = 0;
                self.in_vblank = false;
                ev |= EV_VBLANK_END;
                if self.interlace {
                    self.field = !self.field;
                } else {
                    self.field = false;
                }
            }
        }
        ev
    }

    // ---------------- GP1 ----------------

    pub fn gp1(&mut self, v: u32) {
        let p = v & 0xFF_FFFF;
        match v >> 24 {
            0x00 => {
                self.reset_state();
            }
            0x01 => {
                self.cmd.clear();
                self.mode = Mode::Command;
            }
            0x02 => self.irq = false,
            0x03 => self.disp_on = p & 1 == 0,
            0x04 => self.dma_dir = p & 3,
            0x05 => {
                self.disp_x = p & 0x3FE;
                self.disp_y = (p >> 10) & 0x1FF;
            }
            0x06 => self.h_range = (p & 0xFFF, (p >> 12) & 0xFFF),
            0x07 => self.v_range = (p & 0x3FF, (p >> 10) & 0x3FF),
            0x08 => {
                self.hres1 = p & 3;
                self.vres480 = p & 4 != 0;
                self.pal = p & 8 != 0;
                self.depth24 = p & 0x10 != 0;
                self.interlace = p & 0x20 != 0;
                self.hres2 = p & 0x40 != 0;
            }
            0x10..=0x1F => {
                self.gpuread = match p & 7 {
                    2 => self.win_mx | (self.win_my << 5) | (self.win_ox << 10) | (self.win_oy << 15),
                    3 => (self.da_x1 as u32) | ((self.da_y1 as u32) << 10),
                    4 => (self.da_x2 as u32) | ((self.da_y2 as u32) << 10),
                    5 => ((self.off_x as u32) & 0x7FF) | (((self.off_y as u32) & 0x7FF) << 11),
                    7 => 2,
                    _ => self.gpuread,
                };
            }
            _ => {}
        }
    }

    // ---------------- GPUREAD ----------------

    pub fn read_data(&mut self) -> u32 {
        if self.reading {
            let mut out = 0u32;
            for k in 0..2 {
                let r = &mut self.read;
                let x = (r.x + r.cx) as usize & (VRAM_W - 1);
                let y = (r.y + r.cy) as usize & (VRAM_H - 1);
                let px = self.vram[y * VRAM_W + x] as u32;
                out |= px << (16 * k);
                r.cx += 1;
                if r.cx >= r.w {
                    r.cx = 0;
                    r.cy += 1;
                    if r.cy >= r.h {
                        self.reading = false;
                        break;
                    }
                }
            }
            self.gpuread = out;
        }
        self.gpuread
    }

    // ---------------- GP0 ----------------

    fn cmd_len(c: u32) -> usize {
        let op = c >> 24;
        match op {
            0x02 => 3,
            0x20..=0x3F => {
                let quad = op & 0x08 != 0;
                let tex = op & 0x04 != 0;
                let gour = op & 0x10 != 0;
                let n = if quad { 4 } else { 3 };
                1 + n + if tex { n } else { 0 } + if gour { n - 1 } else { 0 }
            }
            0x40..=0x5F => {
                if op & 0x08 != 0 {
                    usize::MAX // polyline, variable
                } else if op & 0x10 != 0 {
                    4
                } else {
                    3
                }
            }
            0x60..=0x7F => {
                let tex = op & 0x04 != 0;
                let var = (op >> 3) & 3 == 0;
                2 + tex as usize + var as usize
            }
            0x80..=0x9F => 4,
            0xA0..=0xBF => 3,
            0xC0..=0xDF => 3,
            _ => 1,
        }
    }

    pub fn gp0(&mut self, w: u32) {
        match self.mode {
            Mode::Load => {
                self.load_pixel(w as u16);
                if self.mode == Mode::Load {
                    self.load_pixel((w >> 16) as u16);
                }
                return;
            }
            Mode::PolyLine => {
                self.cmd.push(w);
                let gour = (self.cmd[0] >> 24) & 0x10 != 0;
                let n = self.cmd.len();
                // terminator is checked in positions where a vertex (or color in gouraud) starts
                let pos_ok = if gour { n >= 4 && (n - 3) % 2 == 1 } else { n >= 3 };
                if pos_ok && (w & 0xF000_F000) == 0x5000_5000 {
                    self.cmd.pop();
                    self.draw_polyline();
                    self.cmd.clear();
                    self.mode = Mode::Command;
                }
                return;
            }
            Mode::Command => {}
        }
        if self.cmd.is_empty() {
            self.need = Self::cmd_len(w);
            if self.need == usize::MAX {
                self.cmd.push(w);
                self.mode = Mode::PolyLine;
                return;
            }
        }
        self.cmd.push(w);
        if self.cmd.len() >= self.need {
            self.exec();
            self.cmd.clear();
        }
    }

    fn exec(&mut self) {
        self.stat_cmds += 1;
        let c = self.cmd[0];
        let op = c >> 24;
        match op {
            0x00 | 0x03..=0x1E => {}
            0x01 => {}
            0x02 => self.fill_rect(),
            0x1F => {
                if !self.irq {
                    self.irq = true;
                    self.irq_edge = true;
                }
            }
            0x20..=0x3F => self.draw_polygon(),
            0x40..=0x5F => self.draw_line_cmd(),
            0x60..=0x7F => self.draw_rect(),
            0x80..=0x9F => self.copy_vram(),
            0xA0..=0xBF => {
                let (x, y) = (self.cmd[1] & 0x3FF, (self.cmd[1] >> 16) & 0x1FF);
                let w = ((self.cmd[2] & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1;
                let h = (((self.cmd[2] >> 16) & 0xFFFF).wrapping_sub(1) & 0x1FF) + 1;
                self.load = Xfer { x, y, w, h, cx: 0, cy: 0 };
                self.mode = Mode::Load;
            }
            0xC0..=0xDF => {
                let (x, y) = (self.cmd[1] & 0x3FF, (self.cmd[1] >> 16) & 0x1FF);
                let w = ((self.cmd[2] & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1;
                let h = (((self.cmd[2] >> 16) & 0xFFFF).wrapping_sub(1) & 0x1FF) + 1;
                self.read = Xfer { x, y, w, h, cx: 0, cy: 0 };
                self.reading = true;
            }
            0xE1 => {
                self.tp_x = c & 0xF;
                self.tp_y = (c >> 4) & 1;
                self.semi_mode = (c >> 5) & 3;
                self.tex_depth = (c >> 7) & 3;
                self.dither = c & (1 << 9) != 0;
                self.draw_to_display = c & (1 << 10) != 0;
                self.tex_disable = c & (1 << 11) != 0;
                self.flip_x = c & (1 << 12) != 0;
                self.flip_y = c & (1 << 13) != 0;
            }
            0xE2 => {
                self.win_mx = c & 0x1F;
                self.win_my = (c >> 5) & 0x1F;
                self.win_ox = (c >> 10) & 0x1F;
                self.win_oy = (c >> 15) & 0x1F;
            }
            0xE3 => {
                self.da_x1 = (c & 0x3FF) as i32;
                self.da_y1 = ((c >> 10) & 0x1FF) as i32;
            }
            0xE4 => {
                self.da_x2 = (c & 0x3FF) as i32;
                self.da_y2 = ((c >> 10) & 0x1FF) as i32;
            }
            0xE5 => {
                self.off_x = ((c << 21) as i32) >> 21;
                self.off_y = (((c >> 11) << 21) as i32) >> 21;
            }
            0xE6 => {
                self.set_mask = c & 1 != 0;
                self.check_mask = c & 2 != 0;
            }
            _ => {}
        }
    }

    fn load_pixel(&mut self, p: u16) {
        let l = &mut self.load;
        let x = (l.x + l.cx) as usize & (VRAM_W - 1);
        let y = (l.y + l.cy) as usize & (VRAM_H - 1);
        let i = y * VRAM_W + x;
        if !(self.check_mask && self.vram[i] & 0x8000 != 0) {
            self.vram[i] = p | if self.set_mask { 0x8000 } else { 0 };
        }
        l.cx += 1;
        if l.cx >= l.w {
            l.cx = 0;
            l.cy += 1;
            if l.cy >= l.h {
                self.mode = Mode::Command;
            }
        }
    }

    fn fill_rect(&mut self) {
        let c = self.cmd[0];
        let col = rgb24_to_15(c);
        let x0 = self.cmd[1] & 0x3F0;
        let y0 = (self.cmd[1] >> 16) & 0x1FF;
        let w = ((self.cmd[2] & 0x3FF) + 0xF) & !0xF;
        let h = (self.cmd[2] >> 16) & 0x1FF;
        for y in 0..h {
            let yy = ((y0 + y) as usize) & (VRAM_H - 1);
            for x in 0..w {
                let xx = ((x0 + x) as usize) & (VRAM_W - 1);
                self.vram[yy * VRAM_W + xx] = col;
            }
        }
    }

    fn copy_vram(&mut self) {
        let sx = self.cmd[1] & 0x3FF;
        let sy = (self.cmd[1] >> 16) & 0x1FF;
        let dx = self.cmd[2] & 0x3FF;
        let dy = (self.cmd[2] >> 16) & 0x1FF;
        let w = ((self.cmd[3] & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1;
        let h = (((self.cmd[3] >> 16) & 0xFFFF).wrapping_sub(1) & 0x1FF) + 1;
        let mut line = vec![0u16; w as usize];
        for y in 0..h {
            let syy = ((sy + y) as usize) & (VRAM_H - 1);
            let dyy = ((dy + y) as usize) & (VRAM_H - 1);
            for x in 0..w {
                line[x as usize] = self.vram[syy * VRAM_W + (((sx + x) as usize) & (VRAM_W - 1))];
            }
            for x in 0..w {
                let i = dyy * VRAM_W + (((dx + x) as usize) & (VRAM_W - 1));
                if self.check_mask && self.vram[i] & 0x8000 != 0 {
                    continue;
                }
                self.vram[i] = line[x as usize] | if self.set_mask { 0x8000 } else { 0 };
            }
        }
    }

    // ---------------- primitives ----------------

    fn vertex_xy(&self, w: u32) -> (i32, i32) {
        let x = ((w << 21) as i32) >> 21;
        let y = (((w >> 16) << 21) as i32) >> 21;
        (x + self.off_x, y + self.off_y)
    }

    fn draw_polygon(&mut self) {
        let c = self.cmd[0];
        let op = c >> 24;
        let gour = op & 0x10 != 0;
        let quad = op & 0x08 != 0;
        let tex = op & 0x04 != 0;
        let semi = op & 0x02 != 0;
        let raw = op & 0x01 != 0;
        let n = if quad { 4 } else { 3 };

        let mut v = [Vtx::default(); 4];
        let mut idx = 0;
        let mut clut = 0;
        let mut tpage = 0;
        for k in 0..n {
            let col = if k == 0 || !gour {
                c
            } else {
                let w = self.cmd[idx];
                idx += 1;
                w
            };
            if k == 0 {
                idx = 1;
            }
            let (x, y) = self.vertex_xy(self.cmd[idx]);
            idx += 1;
            v[k] = Vtx {
                x,
                y,
                r: (col & 0xFF) as i32,
                g: ((col >> 8) & 0xFF) as i32,
                b: ((col >> 16) & 0xFF) as i32,
                u: 0,
                v: 0,
            };
            if tex {
                let t = self.cmd[idx];
                idx += 1;
                v[k].u = (t & 0xFF) as i32;
                v[k].v = ((t >> 8) & 0xFF) as i32;
                if k == 0 {
                    clut = t >> 16;
                }
                if k == 1 {
                    tpage = t >> 16;
                }
            }
        }

        let mut p = Prim {
            textured: tex,
            gouraud: gour,
            semi,
            raw,
            semi_mode: self.semi_mode,
            tp_x: self.tp_x,
            tp_y: self.tp_y,
            depth: self.tex_depth,
            clut_x: (clut & 0x3F) * 16,
            clut_y: (clut >> 6) & 0x1FF,
            dither: false,
        };
        if tex {
            // polygon texpage updates the global draw mode
            self.tp_x = tpage & 0xF;
            self.tp_y = (tpage >> 4) & 1;
            self.semi_mode = (tpage >> 5) & 3;
            self.tex_depth = (tpage >> 7) & 3;
            self.tex_disable = tpage & (1 << 11) != 0;
            p.tp_x = self.tp_x;
            p.tp_y = self.tp_y;
            p.semi_mode = self.semi_mode;
            p.depth = self.tex_depth;
        }
        p.dither = self.dither && self.opt_dither && (gour || (tex && !raw));
        if !gour && !(tex && !raw) {
            p.dither = false;
        }

        self.triangle(v[0], v[1], v[2], &p);
        if quad {
            self.triangle(v[1], v[2], v[3], &p);
        }
    }

    #[inline(always)]
    fn tex_fetch(&self, p: &Prim, u: i32, v: i32) -> u16 {
        let u = ((u as u32 & 0xFF) & !(self.win_mx * 8)) | ((self.win_ox & self.win_mx) * 8);
        let v = ((v as u32 & 0xFF) & !(self.win_my * 8)) | ((self.win_oy & self.win_my) * 8);
        let bx = p.tp_x * 64;
        let by = p.tp_y * 256;
        let y = ((by + v) as usize) & (VRAM_H - 1);
        match p.depth {
            0 => {
                let w = self.vram[y * VRAM_W + (((bx + u / 4) as usize) & (VRAM_W - 1))];
                let idx = (w >> ((u & 3) * 4)) & 0xF;
                self.vram[(p.clut_y as usize) * VRAM_W + (((p.clut_x + idx as u32) as usize) & (VRAM_W - 1))]
            }
            1 => {
                let w = self.vram[y * VRAM_W + (((bx + u / 2) as usize) & (VRAM_W - 1))];
                let idx = (w >> ((u & 1) * 8)) & 0xFF;
                self.vram[(p.clut_y as usize) * VRAM_W + (((p.clut_x + idx as u32) as usize) & (VRAM_W - 1))]
            }
            _ => self.vram[y * VRAM_W + (((bx + u) as usize) & (VRAM_W - 1))],
        }
    }

    /// Shade and write one pixel. `r,g,b` are 8-bit vertex colors.
    #[inline(always)]
    fn shade_pixel(&mut self, x: i32, y: i32, p: &Prim, r: i32, g: i32, b: i32, u: i32, v: i32) {
        let (mut cr, mut cg, mut cb);
        let mut semi = p.semi;
        if p.textured {
            let t = self.tex_fetch(p, u, v);
            if t == 0 {
                return;
            }
            semi = p.semi && (t & 0x8000 != 0);
            let tr = (t & 0x1F) as i32;
            let tg = ((t >> 5) & 0x1F) as i32;
            let tb = ((t >> 10) & 0x1F) as i32;
            if p.raw {
                cr = tr << 3;
                cg = tg << 3;
                cb = tb << 3;
            } else {
                cr = (tr * r) >> 4;
                cg = (tg * g) >> 4;
                cb = (tb * b) >> 4;
            }
        } else {
            cr = r;
            cg = g;
            cb = b;
        }
        if p.dither {
            let d = DITHER[(y & 3) as usize][(x & 3) as usize];
            cr += d;
            cg += d;
            cb += d;
        }
        let c = ((cr.clamp(0, 255) >> 3) as u16)
            | (((cg.clamp(0, 255) >> 3) as u16) << 5)
            | (((cb.clamp(0, 255) >> 3) as u16) << 10);
        self.plot(x, y, c, semi, p.semi_mode);
    }

    #[inline(always)]
    fn plot(&mut self, x: i32, y: i32, c: u16, semi: bool, mode: u32) {
        let i = (y as usize) * VRAM_W + x as usize;
        let dst = self.vram[i];
        if self.check_mask && dst & 0x8000 != 0 {
            return;
        }
        let mut out = c & 0x7FFF;
        if semi {
            let blend = |b: i32, f: i32| -> i32 {
                match mode {
                    0 => (b + f) >> 1,
                    1 => (b + f).min(31),
                    2 => (b - f).max(0),
                    _ => (b + (f >> 2)).min(31),
                }
            };
            let br = (dst & 0x1F) as i32;
            let bg = ((dst >> 5) & 0x1F) as i32;
            let bb = ((dst >> 10) & 0x1F) as i32;
            let fr = (c & 0x1F) as i32;
            let fg = ((c >> 5) & 0x1F) as i32;
            let fb = ((c >> 10) & 0x1F) as i32;
            out = (blend(br, fr) as u16) | ((blend(bg, fg) as u16) << 5) | ((blend(bb, fb) as u16) << 10);
        }
        if self.set_mask {
            out |= 0x8000;
        }
        self.vram[i] = out;
    }

    fn triangle(&mut self, v0: Vtx, v1: Vtx, v2: Vtx, p: &Prim) {
        let minx = v0.x.min(v1.x).min(v2.x);
        let maxx = v0.x.max(v1.x).max(v2.x);
        let miny = v0.y.min(v1.y).min(v2.y);
        let maxy = v0.y.max(v1.y).max(v2.y);
        if maxx - minx >= 1024 || maxy - miny >= 512 {
            return;
        }
        let area = (v1.x - v0.x) as i64 * (v2.y - v0.y) as i64 - (v1.y - v0.y) as i64 * (v2.x - v0.x) as i64;
        if area == 0 {
            return;
        }
        let (a, b, c) = if area < 0 { (v0, v2, v1) } else { (v0, v1, v2) };

        let x1 = minx.max(self.da_x1);
        let x2 = maxx.min(self.da_x2).min(1023);
        let y1 = miny.max(self.da_y1);
        let y2 = maxy.min(self.da_y2).min(511);
        if x1 > x2 || y1 > y2 {
            return;
        }
        self.stat_pixels += ((x2 - x1 + 1) * (y2 - y1 + 1)) as u64 / 2;

        // Attribute planes in 16.16 fixed point: f(x,y) = f0 + dfdx*(x-x0) + dfdy*(y-y0)
        let dx1 = (v1.x - v0.x) as i64;
        let dy1 = (v1.y - v0.y) as i64;
        let dx2 = (v2.x - v0.x) as i64;
        let dy2 = (v2.y - v0.y) as i64;
        let plane = |f0: i32, f1: i32, f2: i32| -> (i64, i64) {
            let d1 = (f1 - f0) as i64;
            let d2 = (f2 - f0) as i64;
            (((d1 * dy2 - d2 * dy1) << 16) / area, ((d2 * dx1 - d1 * dx2) << 16) / area)
        };
        let gour = p.gouraud && !(v0.r == v1.r && v1.r == v2.r && v0.g == v1.g && v1.g == v2.g && v0.b == v1.b && v1.b == v2.b);
        let tex = p.textured;
        let (rdx, rdy) = if gour { plane(v0.r, v1.r, v2.r) } else { (0, 0) };
        let (gdx, gdy) = if gour { plane(v0.g, v1.g, v2.g) } else { (0, 0) };
        let (bdx, bdy) = if gour { plane(v0.b, v1.b, v2.b) } else { (0, 0) };
        let (udx, udy) = if tex { plane(v0.u, v1.u, v2.u) } else { (0, 0) };
        let (vdx, vdy) = if tex { plane(v0.v, v1.v, v2.v) } else { (0, 0) };
        let at = |f0: i32, ddx: i64, ddy: i64, x: i32, y: i32| -> i64 {
            ((f0 as i64) << 16) + ddx * (x - v0.x) as i64 + ddy * (y - v0.y) as i64 + 0x8000
        };

        // Edge functions with top-left fill rule: inside when e >= 0.
        let edge = |p0: Vtx, q: Vtx| -> (i64, i64, i64) {
            let dx = (q.x - p0.x) as i64;
            let dy = (q.y - p0.y) as i64;
            let bias = if dy < 0 || (dy == 0 && dx > 0) { 0 } else { -1 };
            (dx * (y1 - p0.y) as i64 - dy * (x1 - p0.x) as i64 + bias, -dy, dx)
        };
        let edges = [edge(a, b), edge(b, c), edge(c, a)];
        let mut erow = [edges[0].0, edges[1].0, edges[2].0];

        let simple = !tex && !gour && !p.semi && !p.dither && !self.check_mask;
        for y in y1..=y2 {
            // Solve the span [xs, xe] where all three edge functions are >= 0.
            let mut xs = x1 as i64;
            let mut xe = x2 as i64;
            for k in 0..3 {
                let e0 = erow[k];
                let sx = edges[k].1;
                if sx > 0 {
                    // e0 + sx*(x-x1) >= 0  =>  x >= x1 + ceil(-e0/sx)
                    let n = -e0;
                    let q = if n > 0 { (n + sx - 1) / sx } else { -((-n) / sx) };
                    xs = xs.max(x1 as i64 + q);
                } else if sx < 0 {
                    // x <= x1 + floor(e0 / -sx)
                    let d = -sx;
                    let q = if e0 >= 0 { e0 / d } else { -((-e0 + d - 1) / d) };
                    xe = xe.min(x1 as i64 + q);
                } else if e0 < 0 {
                    xe = xs - 1;
                }
            }
            for k in 0..3 {
                erow[k] += edges[k].2;
            }
            if xs > xe {
                continue;
            }
            let (xs, xe) = (xs as i32, xe as i32);
            if simple {
                let col = ((v0.r >> 3) as u16) | (((v0.g >> 3) as u16) << 5) | (((v0.b >> 3) as u16) << 10)
                    | if self.set_mask { 0x8000 } else { 0 };
                let row = (y as usize) * VRAM_W;
                self.vram[row + xs as usize..=row + xe as usize].fill(col);
                continue;
            }
            let mut r = at(v0.r, rdx, rdy, xs, y);
            let mut g = at(v0.g, gdx, gdy, xs, y);
            let mut bb = at(v0.b, bdx, bdy, xs, y);
            let mut u = at(v0.u, udx, udy, xs, y);
            let mut v = at(v0.v, vdx, vdy, xs, y);
            for x in xs..=xe {
                let (cr, cg, cb) = if gour {
                    ((r >> 16) as i32, (g >> 16) as i32, (bb >> 16) as i32)
                } else {
                    (v0.r, v0.g, v0.b)
                };
                let (tu, tv) = if tex {
                    (((u >> 16) as i32).clamp(0, 255), ((v >> 16) as i32).clamp(0, 255))
                } else {
                    (0, 0)
                };
                self.shade_pixel(x, y, p, cr, cg, cb, tu, tv);
                r += rdx;
                g += gdx;
                bb += bdx;
                u += udx;
                v += vdx;
            }
        }
    }

    fn draw_rect(&mut self) {
        let c = self.cmd[0];
        let op = c >> 24;
        let tex = op & 0x04 != 0;
        let semi = op & 0x02 != 0;
        let raw = op & 0x01 != 0;
        let (x0, y0) = self.vertex_xy(self.cmd[1]);
        let mut idx = 2;
        let (mut u0, mut v0, mut clut) = (0i32, 0i32, 0u32);
        if tex {
            let t = self.cmd[idx];
            idx += 1;
            u0 = (t & 0xFF) as i32;
            v0 = ((t >> 8) & 0xFF) as i32;
            clut = t >> 16;
        }
        let (w, h) = match (op >> 3) & 3 {
            0 => {
                let s = self.cmd[idx];
                ((s & 0x3FF) as i32, ((s >> 16) & 0x1FF) as i32)
            }
            1 => (1, 1),
            2 => (8, 8),
            _ => (16, 16),
        };
        let p = Prim {
            textured: tex,
            gouraud: false,
            semi,
            raw,
            semi_mode: self.semi_mode,
            tp_x: self.tp_x,
            tp_y: self.tp_y,
            depth: self.tex_depth,
            clut_x: (clut & 0x3F) * 16,
            clut_y: (clut >> 6) & 0x1FF,
            dither: false,
        };
        let r = (c & 0xFF) as i32;
        let g = ((c >> 8) & 0xFF) as i32;
        let b = ((c >> 16) & 0xFF) as i32;
        let xs = x0.max(self.da_x1);
        let xe = (x0 + w - 1).min(self.da_x2).min(1023);
        let ys = y0.max(self.da_y1);
        let ye = (y0 + h - 1).min(self.da_y2).min(511);
        for y in ys..=ye {
            let dy = y - y0;
            let tv = if self.flip_y { v0 - dy } else { v0 + dy };
            for x in xs..=xe {
                let dx = x - x0;
                let tu = if self.flip_x { u0 - dx } else { u0 + dx };
                self.shade_pixel(x, y, &p, r, g, b, tu & 0xFF, tv & 0xFF);
            }
        }
    }

    fn draw_line_cmd(&mut self) {
        let c = self.cmd[0];
        let gour = (c >> 24) & 0x10 != 0;
        let (x0, y0) = self.vertex_xy(self.cmd[1]);
        let (c1, xy1) = if gour { (self.cmd[2], self.cmd[3]) } else { (c, self.cmd[2]) };
        let (x1, y1) = self.vertex_xy(xy1);
        let semi = (c >> 24) & 0x02 != 0;
        self.line(x0, y0, c, x1, y1, c1, gour, semi);
    }

    fn draw_polyline(&mut self) {
        let c = self.cmd[0];
        let gour = (c >> 24) & 0x10 != 0;
        let semi = (c >> 24) & 0x02 != 0;
        let mut pts: Vec<(i32, i32, u32)> = Vec::new();
        let mut i = 1;
        let mut col = c;
        let mut first = true;
        while i < self.cmd.len() {
            if gour && !first {
                col = self.cmd[i];
                i += 1;
                if i >= self.cmd.len() {
                    break;
                }
            }
            first = false;
            let (x, y) = self.vertex_xy(self.cmd[i]);
            i += 1;
            pts.push((x, y, col));
        }
        for k in 1..pts.len() {
            let (x0, y0, c0) = pts[k - 1];
            let (x1, y1, c1) = pts[k];
            self.line(x0, y0, c0, x1, y1, c1, gour, semi);
        }
    }

    fn line(&mut self, x0: i32, y0: i32, c0: u32, x1: i32, y1: i32, c1: u32, gour: bool, semi: bool) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        if dx.abs() >= 1024 || dy.abs() >= 512 {
            return;
        }
        let steps = dx.abs().max(dy.abs());
        let comp = |c: u32, s: u32| ((c >> s) & 0xFF) as i32;
        let dith = self.dither && self.opt_dither && gour;
        for k in 0..=steps {
            let (x, y) = if steps == 0 {
                (x0, y0)
            } else {
                (x0 + (dx * k + steps / 2 * dx.signum()) / steps, y0 + (dy * k + steps / 2 * dy.signum()) / steps)
            };
            if x < self.da_x1 || x > self.da_x2 || y < self.da_y1 || y > self.da_y2 || x > 1023 || y > 511 || x < 0 || y < 0 {
                continue;
            }
            let (mut r, mut g, mut b) = (comp(c0, 0), comp(c0, 8), comp(c0, 16));
            if gour && steps > 0 {
                r += (comp(c1, 0) - r) * k / steps;
                g += (comp(c1, 8) - g) * k / steps;
                b += (comp(c1, 16) - b) * k / steps;
            }
            if dith {
                let d = DITHER[(y & 3) as usize][(x & 3) as usize];
                r += d;
                g += d;
                b += d;
            }
            let c = ((r.clamp(0, 255) >> 3) as u16)
                | (((g.clamp(0, 255) >> 3) as u16) << 5)
                | (((b.clamp(0, 255) >> 3) as u16) << 10);
            self.plot(x, y, c, semi, self.semi_mode);
        }
    }

    // ---------------- display output ----------------

    pub fn display_size(&self) -> (u32, u32) {
        let w = if self.hres2 {
            368
        } else {
            match self.hres1 {
                0 => 256,
                1 => 320,
                2 => 512,
                _ => 640,
            }
        };
        let base_h = if self.pal { 256 } else { 240 };
        let lines = self.v_range.1.saturating_sub(self.v_range.0);
        let mut h = if lines > 0 && lines <= 300 { lines } else { base_h };
        if self.interlace && self.vres480 {
            h *= 2;
        }
        (w, h)
    }

    /// Render the visible display area into an RGBA buffer.
    pub fn render(&self, out: &mut Vec<u8>, full_vram: bool) -> (u32, u32) {
        if full_vram {
            out.resize(VRAM_W * VRAM_H * 4, 0);
            for (i, &p) in self.vram.iter().enumerate() {
                let o = i * 4;
                out[o] = ((p & 0x1F) << 3) as u8;
                out[o + 1] = (((p >> 5) & 0x1F) << 3) as u8;
                out[o + 2] = (((p >> 10) & 0x1F) << 3) as u8;
                out[o + 3] = 255;
            }
            return (VRAM_W as u32, VRAM_H as u32);
        }
        let (w, h) = self.display_size();
        out.resize((w * h * 4) as usize, 0);
        if !self.disp_on {
            out.iter_mut().enumerate().for_each(|(i, b)| *b = if i % 4 == 3 { 255 } else { 0 });
            return (w, h);
        }
        for y in 0..h {
            let vy = ((self.disp_y + y) as usize) & (VRAM_H - 1);
            let row = vy * VRAM_W;
            let ob = (y * w * 4) as usize;
            if self.depth24 {
                let base = (self.disp_x as usize) * 2;
                for x in 0..w as usize {
                    let bo = base + x * 3;
                    let byte = |k: usize| -> u8 {
                        let pos = bo + k;
                        let px = self.vram[row + ((pos / 2) & (VRAM_W - 1))];
                        if pos & 1 == 0 { px as u8 } else { (px >> 8) as u8 }
                    };
                    let o = ob + x * 4;
                    out[o] = byte(0);
                    out[o + 1] = byte(1);
                    out[o + 2] = byte(2);
                    out[o + 3] = 255;
                }
            } else {
                for x in 0..w as usize {
                    let p = self.vram[row + ((self.disp_x as usize + x) & (VRAM_W - 1))];
                    let o = ob + x * 4;
                    let r = (p & 0x1F) as u8;
                    let g = ((p >> 5) & 0x1F) as u8;
                    let b = ((p >> 10) & 0x1F) as u8;
                    out[o] = (r << 3) | (r >> 2);
                    out[o + 1] = (g << 3) | (g >> 2);
                    out[o + 2] = (b << 3) | (b >> 2);
                    out[o + 3] = 255;
                }
            }
        }
        (w, h)
    }

    // ---------------- save-state helpers ----------------
    pub fn vram_bytes(&self) -> Vec<u8> {
        self.vram.iter().flat_map(|p| p.to_le_bytes()).collect()
    }
}

fn rgb24_to_15(c: u32) -> u16 {
    let r = ((c & 0xFF) >> 3) as u16;
    let g = (((c >> 8) & 0xFF) >> 3) as u16;
    let b = (((c >> 16) & 0xFF) >> 3) as u16;
    r | (g << 5) | (b << 10)
}

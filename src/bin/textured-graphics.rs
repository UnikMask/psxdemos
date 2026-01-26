#![no_std]
#![no_main]

use psx::{
    Framebuffer, LoadedTIM, TextBox, dma, dprintln,
    gpu::{
        Color, Packet, TexCoord, TexPage, Vertex, VideoMode,
        primitives::{DrawModeTexPage, Sprt, Tile},
    },
    hw::gpu::GP0Command,
    include_tim,
};
use thiserror_no_std::Error;

const RES_X: i16 = 320;
const RES_Y: i16 = 240;
const VIDEO_MODE: VideoMode = VideoMode::NTSC;
const BG_COLOR: Color = Color {
    red: 63,
    green: 0,
    blue: 167,
};
const DBG_TEXT_OFFSET: (i16, i16) = (8, 8);

const OT_SIZE: usize = 8;
const BUF_SIZE: usize = 32768;

struct MainState {
    fb: Framebuffer,
    gpu_dma: dma::GPU,
    otc_dma: dma::OTC,
    txt: TextBox,
}

// Initialize main state
fn init() -> MainState {
    let (buf0, buf1) = ((0, 0), (0, RES_Y));
    let res = (RES_X, RES_Y);
    let mut fb = Framebuffer::new(buf0, buf1, res, VIDEO_MODE, None).expect("Failed??");
    fb.set_bg_color(BG_COLOR);

    let txt = fb.load_default_font().new_text_box(DBG_TEXT_OFFSET, res);
    MainState {
        fb,
        gpu_dma: dma::GPU::new(),
        otc_dma: dma::OTC::new(),
        txt,
    }
}

struct GraphicsState {
    otc: [[Packet<()>; OT_SIZE]; 2],
    buffer: [GraphicsBuffer<BUF_SIZE>; 2],
    swapped: bool,
}

struct GraphicsBuffer<const LEN: usize> {
    buf: [u8; LEN],
    index: usize,
}

#[derive(Debug, Error)]
#[error(
    "Couldn't add packet to buffer - max buffer length is {max_len}, but current index is {index} and packet length is {packet_len}."
)]
struct GraphicsBufferFullError {
    index: usize,
    packet_len: usize,
    max_len: usize,
}

impl<const LEN: usize> GraphicsBuffer<LEN> {
    pub const fn new() -> Self {
        Self {
            buf: [0; LEN],
            index: 0,
        }
    }

    pub fn new_primitive<T: GP0Command>(
        &mut self,
    ) -> Result<&mut Packet<T>, GraphicsBufferFullError> {
        let packet_len = core::mem::size_of::<Packet<T>>();
        let index_aligned = self.index.next_multiple_of(packet_len);

        if LEN <= index_aligned + packet_len {
            return Err(GraphicsBufferFullError {
                index: self.index,
                packet_len,
                max_len: LEN,
            });
        }

        let prim_mut_ref = unsafe {
            (&raw mut self.buf)
                .add(index_aligned)
                .cast::<Packet<T>>()
                .as_mut()
                .expect("Null pointer on index access!")
        };
        self.index = index_aligned + packet_len;
        Ok(prim_mut_ref)
    }

    pub unsafe fn new_primitive_unchecked<T: GP0Command>(&mut self) -> &mut Packet<T> {
        let packet_len = core::mem::size_of::<Packet<T>>();
        let index_aligned = self.index.next_multiple_of(packet_len);

        let prim_mut_ref = unsafe {
            (&raw mut self.buf)
                .add(index_aligned)
                .cast::<Packet<T>>()
                .as_mut()
                .expect("Null pointer on index access!")
        };
        self.index = index_aligned + packet_len;
        prim_mut_ref
    }

    pub fn reset(&mut self) {
        self.index = 0;
    }
}

struct GraphicsEnv<'a> {
    otc: &'a mut [Packet<()>; OT_SIZE],
    buffer: &'a mut GraphicsBuffer<BUF_SIZE>,
}

fn get_disp_and_draw(graphics: &'_ mut GraphicsState) -> (GraphicsEnv<'_>, GraphicsEnv<'_>) {
    let GraphicsState {
        otc,
        buffer,
        swapped,
    } = graphics;
    let [otc_a, otc_b] = otc;
    let [buf_a, buf_b] = buffer;
    let (a, b) = (
        GraphicsEnv {
            otc: otc_a,
            buffer: buf_a,
        },
        GraphicsEnv {
            otc: otc_b,
            buffer: buf_b,
        },
    );
    if *swapped { (a, b) } else { (b, a) }
}

// Initialize graphics - i.e. the ordering tables and primitive buffers
fn init_graphics() -> GraphicsState {
    GraphicsState {
        otc: [const { [const { Packet::new(()) }; OT_SIZE] }; 2],
        buffer: [const { GraphicsBuffer::<BUF_SIZE>::new() }; 2],
        swapped: false,
    }
}

struct LevelState {
    frame_no: i16,
    texture_stone: LoadedTIM,
}

// Load the level data
fn load_level(fb: &mut Framebuffer) -> LevelState {
    // Load the textures
    let textures_stones = include_tim!("../../resources/stones_useable.tim");

    LevelState {
        frame_no: 0,
        texture_stone: fb.load_tim(textures_stones),
    }
}

/// Update things - logic written here
fn update(graphics: &mut GraphicsEnv, txt: &mut TextBox, state: &mut LevelState) {
    graphics.buffer.reset();

    (0..8).for_each(|i| {
        dprintln!(
            txt,
            "OTC {i}: {:x} -> {:x}",
            &raw const graphics.otc[i] as u32,
            graphics.otc[i].header_address()
        );
    });

    // Draw a sprite primitive
    let sprt = graphics
        .buffer
        .new_primitive::<Sprt>()
        .expect("No new sprite!");
    sprt.contents = Sprt::new();
    sprt.contents.set_offset(Vertex(48, 48));
    sprt.contents.set_size(Vertex(64, 64));
    sprt.contents.set_tex_coord(TexCoord { x: 0, y: 0 });
    if let Some(clut) = state.texture_stone.clut {
        sprt.contents.set_clut(clut);
    }
    graphics.otc[1].insert_packet(sprt);

    dprintln!(txt, "texpage: {:b}", unsafe {
        core::mem::transmute::<TexPage, u16>(state.texture_stone.tex_page)
    });

    // Add the TPage primitive next
    let tpage = graphics
        .buffer
        .new_primitive::<DrawModeTexPage>()
        .expect("Getting tex page failed!");
    tpage.contents = DrawModeTexPage::new();
    tpage.contents.set_tex_page(state.texture_stone.tex_page);
    graphics.otc[1].insert_packet(tpage);

    // Add tiles
    for i in 0..8 {
        let orig_pos = (i as i16) * 40;
        let lines = (state.frame_no + orig_pos) / 280;
        let x_offset = if lines % 2 == 0 {
            (orig_pos + state.frame_no) % 280
        } else {
            280 - ((orig_pos + state.frame_no) % 280)
        };
        let tile = graphics.buffer.new_primitive::<Tile>().expect("No!?");
        tile.contents = Tile::new();
        tile.contents.set_offset(Vertex(x_offset, (i as i16) * 30));
        tile.contents.set_size(Vertex(40, 30));
        tile.contents.set_color(Color {
            red: 255,
            green: 255,
            blue: 0,
        });
        graphics.otc[2].insert_packet(tile);
    }

    state.frame_no = (state.frame_no + 1) % 560;
}

#[unsafe(no_mangle)]
fn main() {
    let MainState {
        mut fb,
        mut gpu_dma,
        mut otc_dma,
        mut txt,
    } = init();

    // Set up graphics
    let mut graphics_state = init_graphics();

    // Set up level
    let mut level_state = load_level(&mut fb);

    loop {
        graphics_state.swapped = !graphics_state.swapped;
        let (disp, mut draw) = get_disp_and_draw(&mut graphics_state);

        gpu_dma.send_list_and(&disp.otc[2], || {
            let draw_otc =
                unsafe { core::mem::transmute::<&mut [Packet<()>], &mut [u32]>(draw.otc) };
            otc_dma.send_reverse(draw_otc).expect("OTC DMA failed!");
            txt.reset();
            update(&mut draw, &mut txt, &mut level_state);
        });

        fb.draw_sync();
        fb.wait_vblank();
        fb.dma_swap(&mut gpu_dma);
    }
}

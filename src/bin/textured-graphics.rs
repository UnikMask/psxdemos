#![no_std]
#![no_main]

use psx::{
    Framebuffer, TextBox, dma,
    gpu::{Color, Packet, VideoMode, primitives::Tile},
};

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
const BUF_SIZE: usize = 16;

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
    buffer: [[Packet<Tile>; BUF_SIZE]; 2],
    swapped: bool,
}

struct GraphicsEnv<'a> {
    otc: &'a mut [Packet<()>; OT_SIZE],
    buffer: &'a mut [Packet<Tile>; BUF_SIZE],
}

fn get_disp_and_draw(graphics: &mut GraphicsState) -> (GraphicsEnv, GraphicsEnv) {
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
        buffer: [const { [const { Packet::new(Tile::new()) }; BUF_SIZE] }; 2],
        swapped: false,
    }
}

#[derive(Default)]
struct LevelState {
    frame_no: i16,
}

/// Update things - logic written here
fn update(graphics: GraphicsEnv, txt: &mut TextBox, state: &mut LevelState) {}

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
    let mut level_state = LevelState::default();

    loop {
        graphics_state.swapped = !graphics_state.swapped;
        let (disp, draw) = get_disp_and_draw(&mut graphics_state);

        gpu_dma.send_list_and(&disp.otc[disp.otc.len() - 1], || {
            otc_dma
                .send_reverse(unsafe {
                    core::mem::transmute::<&mut [Packet<()>; OT_SIZE], &mut [u32; OT_SIZE]>(
                        draw.otc,
                    )
                })
                .expect("OTC DMA failed!");
            txt.reset();
            update(draw, &mut txt, &mut level_state);
        });

        fb.draw_sync();
        fb.wait_vblank();
        fb.dma_swap(&mut gpu_dma);
    }
}

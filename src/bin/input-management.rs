#![no_std]
#![no_main]

use psx::{
    Framebuffer, LoadedTIM, TextBox, dma,
    gpu::{
        Bpp, Color, Packet, TexColor, TexCoord, Vertex, VideoMode,
        primitives::{DrawModeTexPage, Sprt},
    },
    hw::gpu::GP0Command,
    include_tim,
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
const BUF_SIZE: usize = 32768;

struct MainState {
    fb: Framebuffer,
    gpu_dma: dma::GPU,
    otc_dma: dma::OTC,
    stdout_tim: LoadedTIM,
    txt: TextBox,
}

struct GraphicsState {
    otc: [[Packet<()>; OT_SIZE]; 2],
    buffer: [[u8; BUF_SIZE]; 2],
    swapped: bool,
}

struct GraphicsEnv<'a> {
    otc: &'a mut [Packet<()>; OT_SIZE],
    buffer: &'a mut [u8; BUF_SIZE],
}

fn new_prim<'a, T: GP0Command>(buf: &'a mut [u8], index: &mut usize) -> &'a mut Packet<T> {
    let prim = unsafe {
        (&raw mut buf[*index])
            .cast::<Packet<T>>()
            .as_mut()
            .expect("Null pointer to address!")
    };
    *index += core::mem::size_of::<Packet<T>>();
    prim
}

// Initialize main state
fn init() -> MainState {
    let (buf0, buf1) = ((0, 0), (0, RES_Y));
    let res = (RES_X, RES_Y);
    let mut fb = Framebuffer::new(buf0, buf1, res, VIDEO_MODE, None).expect("Failed??");
    fb.set_bg_color(BG_COLOR);

    let stdout_tim = fb.load_default_font();
    let txt: TextBox = stdout_tim.new_text_box(DBG_TEXT_OFFSET, res);
    MainState {
        fb,
        gpu_dma: dma::GPU::new(),
        otc_dma: dma::OTC::new(),
        stdout_tim,
        txt,
    }
}

// Initialize graphics - i.e. the ordering tables and primitive buffers
fn init_graphics() -> GraphicsState {
    GraphicsState {
        otc: [const { [const { Packet::new(()) }; OT_SIZE] }; 2],
        buffer: [[0; BUF_SIZE]; 2],
        swapped: false,
    }
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

struct LevelState {
    input_acc: InputAccumulator,
    texture_stone: LoadedTIM,
}

// Load the level data
fn load_level(fb: &mut Framebuffer) -> LevelState {
    // Load the textures
    let textures_stones = include_tim!("../../resources/stones_useable.tim");

    LevelState {
        texture_stone: fb.load_tim(textures_stones),
        input_acc: InputAccumulator {
            x_pressed: 0,
            sq_pressed: 0,
            movement: Vertex(128, 88),
        },
    }
}

struct InputAccumulator {
    x_pressed: i16,
    sq_pressed: i16,
    movement: Vertex,
}

#[unsafe(no_mangle)]
fn main() {
    let MainState {
        mut fb,
        mut gpu_dma,
        mut otc_dma,
        mut txt,
        stdout_tim,
    } = init();

    // Set up graphics
    let level_state = load_level(&mut fb);
    let mut graphics_state = init_graphics();

    loop {
        graphics_state.swapped = !graphics_state.swapped;
        let (disp, draw) = get_disp_and_draw(&mut graphics_state);

        // Reset the ordering table
        let draw_otc = unsafe { core::mem::transmute::<&mut [Packet<()>], &mut [u32]>(draw.otc) };

        // Display previous table, set up drawing for current DMA
        gpu_dma.send_list_and(&disp.otc[OT_SIZE - 1], || {
            otc_dma.send_reverse(draw_otc).expect("OTC DMA failed!");
            txt.reset();

            // Reset draw mode after sprites
            let mut index = 0;
            let stdout_draw = new_prim::<DrawModeTexPage>(draw.buffer, &mut index);
            *stdout_draw = Packet::new(DrawModeTexPage::from(
                stdout_tim.tex_page,
                Bpp::Bits4,
                false,
                true,
            ));
            draw.otc[0].insert_packet(stdout_draw);

            // Set up sprite
            let sprt = new_prim::<Sprt>(draw.buffer, &mut index);
            *sprt = Packet::new(Sprt::new());
            sprt.contents.set_offset(level_state.input_acc.movement);
            sprt.contents.set_size(Vertex(64, 64));
            sprt.contents.set_tex_coord(TexCoord { x: 0, y: 0 });
            sprt.contents.set_color(TexColor {
                red: 128,
                green: 128,
                blue: 128,
            });
            if let Some(clut) = level_state.texture_stone.clut {
                sprt.contents.set_clut(clut);
            }
            draw.otc[1].insert_packet(sprt);

            // Add the TPage primitive for rocks next
            let tpage = new_prim::<DrawModeTexPage>(draw.buffer, &mut index);
            *tpage = Packet::new(DrawModeTexPage::from(
                level_state.texture_stone.tex_page,
                Bpp::Bits8,
                false,
                true,
            ));
            draw.otc[1].insert_packet(tpage);
        });

        fb.draw_sync();
        fb.wait_vblank();
        fb.dma_swap(&mut gpu_dma);
    }
}

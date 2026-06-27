//! Input management demo
//! For sake of learning, this will be using
//! the BIOS functions and not the SDK provided
//! gamepad controls

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]
use core::{arch::asm, mem::MaybeUninit, ptr::read_volatile};

use psx::{
    Framebuffer, LoadedTIM, TextBox, dma, dprintln,
    gpu::{
        Bpp, Color, Packet, TexColor, TexCoord, Vertex, VideoMode,
        primitives::{DrawModeTexPage, Sprt},
    },
    hw::gpu::GP0Command,
    include_tim, println,
    sys::kernel::{psx_change_clear_pad, psx_change_clear_rcnt, psx_init_pad, psx_start_pad},
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
const BUF_SIZE: usize = 32_768;
const PAD_BUF_SIZE: usize = 34;

// Gamepad consts
const PAD_UP: u16 = 0x10;
const PAD_RIGHT: u16 = 0x20;
const PAD_DOWN: u16 = 0x40;
const PAD_LEFT: u16 = 0x80;

static mut PAD_BUFFER: [MaybeUninit<[u32; PAD_BUF_SIZE]>; 2] = [MaybeUninit::uninit(); 2];

struct MainState {
    // Video framebuffer
    fb: Framebuffer,

    // DMA channels
    gpu_dma: dma::GPU,
    otc_dma: dma::OTC,

    // Debug related text and TIM
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

fn init_input() {
    // Initialize gamepad buffer - 0xff value made s.t. program doesn't
    // process faulty input on empty buffer
    unsafe {
        // Initialize the PAD on the buffer we made
        psx_init_pad(
            (&raw mut PAD_BUFFER[0]).cast(),
            34,
            (&raw mut PAD_BUFFER[1]).cast(),
            34,
        )
    };
    unsafe {
        (0..=1).for_each(|i| {
            (&raw mut PAD_BUFFER[i])
                .cast::<u32>()
                .write_volatile(0xffff_ffff);
            (&raw mut PAD_BUFFER[i])
                .cast::<u32>()
                .add(1)
                .write_volatile(0x8080_8080);
        });

        // Edit breakpoint memory
        psx_start_pad();
        asm!("nop"); // For funsies lmao
        psx_change_clear_pad(0);
        psx_change_clear_rcnt(3, false);
    }; //start  gamepad polling
}

#[unsafe(no_mangle)]
fn main() {
    // Initialize input before everything else
    let MainState {
        mut fb,
        mut gpu_dma,
        mut otc_dma,
        mut txt,
        stdout_tim,
    } = init();
    init_input();
    unsafe { (0x8000aaa0 as *mut u32).write_volatile(0x33) };

    // Set up graphics
    let mut graphics_state = init_graphics();
    let mut level_state = load_level(&mut fb);
    let mut i: u32 = 0;

    unsafe { (0x8000aaa0 as *mut u32).write_volatile(0x35) };
    loop {
        graphics_state.swapped = !graphics_state.swapped;
        let (disp, draw) = get_disp_and_draw(&mut graphics_state);

        // TODO: Replace with drawop
        gpu_dma.send_list_and(&disp.otc[OT_SIZE - 1], || {
            // Reset the ordering table
            let draw_otc =
                unsafe { core::mem::transmute::<&mut [Packet<()>], &mut [u32]>(draw.otc) };
            i += 1;

            otc_dma.send_reverse(draw_otc).expect("OTC DMA failed!");

            // Listen to user input
            let buttons = unsafe { read_volatile((&raw const PAD_BUFFER[0]).cast::<u16>().add(1)) };
            println!("Buttons: {buttons:x}");
            if (buttons & PAD_UP) > 0 {
                level_state.input_acc.movement.1 += 1;
            }
            if (buttons & PAD_DOWN) > 0 {
                level_state.input_acc.movement.1 -= 1;
            }
            if (buttons & PAD_LEFT) > 0 {
                level_state.input_acc.movement.0 += 1;
            }
            if (buttons & PAD_RIGHT) > 0 {
                level_state.input_acc.movement.0 -= 1;
            }

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
                red: 128 + (i % 256) as u8,
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
        if !fb.wait_vblank() {
            unsafe { (0x8000aaa0 as *mut u32).write_volatile(0x36) };
            println!("VSync() timeout!");
            unsafe {
                psx_change_clear_pad(0);
                psx_change_clear_rcnt(3, false);
            }
        } else {
            unsafe { (0x8000aaa0 as *mut u32).write_volatile(0x37) };
        }
        fb.dma_swap(&mut gpu_dma);
    }
}

#![no_std]
#![no_main]

use psx::{
    Framebuffer, dprintln,
    gpu::{Color, VideoMode},
};

#[unsafe(no_mangle)]
fn main() {
    // Set the buffer locations on VRAM
    let (buf0, buf1) = ((0, 0), (320, 0));
    let res = (320, 240); // Framebuffer resolution
    let txt_offset = (8, 8); // Offset bw
    let mut fb = Framebuffer::new(buf0, buf1, res, VideoMode::NTSC, None).expect("Failed??");
    // let tim = fb.load_default_font();
    // let mut txt = tim.new_text_box(txt_offset, res); // Make text box take
    // whole framebuffer resolution

    loop {
        // txt.reset();
        // dprintln!(txt, "Hello World!");
        fb.set_bg_color(Color {
            red: 63,
            green: 0,
            blue: 127,
        });
        fb.draw_sync();
        fb.wait_vblank();
        fb.swap();
    }
}

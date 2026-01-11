#![no_std]
#![no_main]

use psx::{Framebuffer, dprintln, gpu::VideoMode};

#[unsafe(no_mangle)]
fn main() {
    let (buf0, buf1) = ((0, 0), (0, 240));
    let res = (320, 240);
    let txt_offset = (0, 8);
    let mut fb = Framebuffer::new(buf0, buf1, res, VideoMode::NTSC, None).expect("Failed??");
    let tim = fb.load_default_font();
    let mut txt = tim.new_text_box(txt_offset, res);

    loop {
        txt.reset();
        dprintln!(txt, "Hello World!");
        fb.draw_sync();
        fb.wait_vblank();
        fb.swap();
    }
}

#![no_std]
#![no_main]

use psx::{
    Framebuffer,
    dma::{self},
    dprintln,
    gpu::{Color, Packet, Vertex, VideoMode, primitives::Tile},
};

#[unsafe(no_mangle)]
fn main() {
    // Set buffer again
    // Set the buffer locations on VRAM
    let (buf0, buf1) = ((0, 0), (0, 240));
    let res = (320, 240); // Framebuffer resolution
    let txt_offset = (8, 8); // Offset bw
    let mut fb = Framebuffer::new(buf0, buf1, res, VideoMode::NTSC, None).expect("Failed??");
    fb.set_bg_color(Color {
        red: 63,
        green: 0,
        blue: 127,
    });
    let font_tim = fb.load_default_font();
    let mut txt = font_tim.new_text_box(txt_offset, res);
    let mut gpu_dma = dma::GPU::new();
    let mut otc_dma = dma::OTC::new(); // DMA channel for OTC
    let mut frame_no = 0;

    // Ordering tables
    let mut otc: [[Packet<()>; 8]; 2] = [const { [const { Packet::new(()) }; 8] }; 2];

    // Following Lameguy64's tutorials here
    let mut packets = [const { [const { Packet::new(Tile::new()) }; 8] }; 2];

    // Main loop
    let mut swapped = false;
    loop {
        // Swap draw buffer and display buffer
        swapped = !swapped;
        let [ref mut draw, ref mut disp] = otc;
        let [draw, disp] = if swapped { [draw, disp] } else { [disp, draw] };
        let packets = if swapped {
            &mut packets[1]
        } else {
            &mut packets[0]
        };

        // Send display list while setting up logic for next line of content
        gpu_dma.send_list_and(&disp[disp.len() - 1], || {
            // Reset text and print needed stuff
            txt.reset();
            dprintln!(txt, "Frame no: {frame_no}");
            let draw_otc = unsafe { core::mem::transmute::<&mut [Packet<()>], &mut [u32]>(draw) };
            otc_dma.send_reverse(draw_otc).expect("OTC DMA failed!"); // Clear ordering table
            (0..8).for_each(|i| {
                dprintln!(
                    txt,
                    "OTC {i}: {:x} -> {:x}",
                    &raw const draw[i] as u32,
                    draw[i].header_address()
                );
            });
            //
            // Clear ordering table with DMA
            // Set up the tiles
            for (i, tile) in packets.iter_mut().enumerate() {
                let orig_pos = (i as i16) * 40;
                let lines = (frame_no + orig_pos) / 280;
                let x_offset = if lines % 2 == 0 {
                    (orig_pos + frame_no) % 280
                } else {
                    280 - ((orig_pos + frame_no) % 280)
                };
                tile.contents.set_offset(Vertex(x_offset, (i as i16) * 30));
                tile.contents.set_size(Vertex(40, 30));
                tile.contents.set_color(Color {
                    red: 255,
                    green: 255,
                    blue: 0,
                });
                draw[2].insert_packet(tile);
            }
        });

        // Wait for GPU to finish drawing
        fb.draw_sync();
        fb.wait_vblank();

        // Switch to next buffer
        fb.dma_swap(&mut gpu_dma);
        frame_no = (frame_no + 1) % 560;
    }
}

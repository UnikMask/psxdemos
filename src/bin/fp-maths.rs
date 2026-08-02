//! Fixed Point Maths demo
//! Based on [Lameguy64's tutorials](https://www.neperos.com/article/smfhgyb39f6bdee3)

#![no_std]
#![no_main]
#![feature(const_trait_impl)]
#![feature(asm_experimental_arch)]

use core::{
    arch::asm,
    fmt::Display,
    iter::Sum,
    mem::MaybeUninit,
    ops::{Add, Div, Mul, Range, Rem, Shr, Sub},
    ptr::read_volatile,
};

use psx::{
    Framebuffer, IndirectMode, LoadedTIM, TextBox, dma, dprintln,
    gpu::{
        Bpp, Color, Packet, TexCoord, TexPage, Vertex, VideoMode,
        primitives::{PolyF3, PolyFT4},
    },
    hw::gpu::GP0Command,
    include_tim,
    sys::kernel::{self, psx_change_clear_pad, psx_change_clear_rcnt, psx_init_pad, psx_start_pad},
};

// General constants for video setup
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

const SEED: u32 = 1039;

/// Main runtime state. Keeps track of DMA and framebuffers.
struct MainState {
    fb: Framebuffer,

    // Debug related text and TIM
    txt: TextBox<IndirectMode<800>>,
}

/// State and lifetime of the current graphics context.
/// Independent of framebuffer.
struct GraphicsState {
    otc: [[Packet<()>; OT_SIZE]; 2],
    buffer: [[u8; BUF_SIZE]; 2],
    swapped: bool,
}

/// Graphic variable environment for the current update loop.
/// References the graphics state.
struct GraphicsEnv<'a> {
    otc: &'a mut [Packet<()>; OT_SIZE],
    index: usize,
    buffer: &'a mut [u8; BUF_SIZE],
}

/// Get the display and draw graphics environment - pass the display environment to the GPU via
/// DMA command, and write on the draw environment for next frame display.
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
            index: 0,
            buffer: buf_a,
        },
        GraphicsEnv {
            otc: otc_b,
            index: 0,
            buffer: buf_b,
        },
    );
    if *swapped { (a, b) } else { (b, a) }
}

/// Add a new primitive to the primitive buffer and insert it to the OTC.
fn add_prim<'a, 'b, T: GP0Command>(env: &'a mut GraphicsEnv<'b>, prim: T, otc_index: usize) {
    let prim_location = unsafe {
        (&raw mut env.buffer[env.index])
            .cast::<Packet<T>>()
            .as_mut()
            .expect("Null pointer to address!")
    };
    env.index += core::mem::size_of::<Packet<T>>();
    *prim_location = Packet::<T>::new(prim);
    env.otc[otc_index].insert_packet(prim_location);
}

// Initialize main state
fn init() -> MainState {
    let (buf0, buf1) = ((0, 0), (0, RES_Y));
    let res = (RES_X, RES_Y);
    let mut fb = Framebuffer::new(buf0, buf1, res, VIDEO_MODE, None).expect("Failed??");
    fb.set_bg_color(BG_COLOR);

    let stdout_tim = fb.load_default_font();
    let txt = TextBox::<IndirectMode<_>>::from_loaded_tim(&stdout_tim, DBG_TEXT_OFFSET, res);

    MainState { fb, txt }
}

// Initialize graphics - i.e. the ordering tables and primitive buffers
fn init_graphics() -> GraphicsState {
    GraphicsState {
        otc: [const { [const { Packet::new(()) }; OT_SIZE] }; 2],
        buffer: [[0; BUF_SIZE]; 2],
        swapped: false,
    }
}

///////////////////////////
// Level-local constants //
///////////////////////////

struct LevelState {
    // Input accumulator for moving our sprites
    transforms: [SpriteTransform; 2],
    // Input velocities applied to transforms on physics updates
    velocities: [SpriteTransform; 2],
    // Textures
    textures: [LoadedTIM; 1],
}

/// Transform component of the controlled sprite
#[derive(Default, Clone, Copy)]
struct SpriteTransform {
    position: (Fixed32, Fixed32),
    rotation: Fixed32,
}

///////////////
// Main Loop //
///////////////

#[unsafe(no_mangle)]
fn main() {
    // Initialize main state and input listening on the BIOS
    let MainState { mut fb, mut txt } = init();
    init_input();

    let rng = Rng::new(SEED);

    // Set up graphics state
    let mut graphics_state = init_graphics();

    // No need to load level - just create an empty struct
    let mut level = LevelState {
        transforms: [
            SpriteTransform {
                position: (Fixed32::from_i16(160), Fixed32::from_i16(120)),
                rotation: Fixed32(0),
            },
            SpriteTransform {
                position: (
                    Fixed32::from_i16(rng.rand_range(20..300) as i16),
                    Fixed32::from_i16(rng.rand_range(20..220) as i16),
                ),
                rotation: Fixed32(rng.rand_range_u32(0..4096 * 360) as i32),
            },
        ],
        velocities: [
            SpriteTransform::default(),
            SpriteTransform {
                position: (
                    Fixed32(rng.rand_range_u32(0..4096) as i32),
                    Fixed32(rng.rand_range_u32(0..4096) as i32),
                ),
                rotation: Fixed32(rng.rand_range_u32(0..2 * 4096) as i32),
            },
        ],
        textures: [fb.load_tim(include_tim!("../../resources/stones_useable.tim"))],
    };

    // Display loop
    let (mut gpu_dma, mut otc_dma) = (dma::GPU::new(), dma::OTC::new());
    loop {
        graphics_state.swapped = !graphics_state.swapped;
        let (disp, mut draw) = get_disp_and_draw(&mut graphics_state);

        gpu_dma.send_list_and(&disp.otc[OT_SIZE - 1], || {
            // Reset text and ordering table
            txt.reset();
            let draw_otc =
                unsafe { core::mem::transmute::<&mut [Packet<()>], &mut [u32]>(draw.otc) };
            otc_dma.send_reverse(draw_otc).expect("OTC DMA failed!");

            // Update transform using user input
            update_transform(&mut level.transforms[0], &mut level.velocities[0], unsafe {
                read_volatile((&raw const PAD_BUFFER[0]).cast::<u16>().add(1))
            });
            update_physics(&mut level.transforms[0], &mut level.velocities[0]);
            update_physics(&mut level.transforms[1], &mut level.velocities[1]);

            // Print sprite location
            dprintln!(
                txt,
                "Position: ({}, {})",
                level.transforms[0].position.0,
                level.transforms[0].position.1
            );
            dprintln!(txt, "Rotation: {} degrees", level.transforms[0].rotation);

            let player_tri = [Vertex(0, -20), Vertex(10, 20), Vertex(-10, 20)];

            // Generate player triangle
            let mut player = PolyF3::new();
            player.set_color(Color {
                red: 255,
                green: 255,
                blue: 0,
            });

            // Convert rotation in degrees to radians
            let SpriteTransform {
                position: (a_x, a_y),
                rotation: arrow_rot,
            } = level.transforms[0];
            let SpriteTransform {
                position: (c_x, c_y),
                rotation: cube_rot,
            } = level.transforms[1];

            // Perform matrix rotation on the player vertices
            let arrow_vertices = player_tri.map(|Vertex(vx, vy)| {
                Vertex(
                    // X position
                    (Fixed32::from_i16(vx) * acos(arrow_rot)
                        - Fixed32::from_i16(vy) * asin(arrow_rot)
                        + a_x)
                        .to_i16(),
                    // Y position
                    (Fixed32::from_i16(vx) * asin(arrow_rot)
                        + Fixed32::from_i16(vy) * acos(arrow_rot)
                        + a_y)
                        .to_i16(),
                )
            });
            player.set_vertices(arrow_vertices);
            add_prim::<PolyF3>(&mut draw, player, 2); // Insert player primitive to OTC index 2

            // Generate textured cube structure and texture
            let mut cube = PolyFT4::new();
            let tex_page = unsafe {
                core::mem::transmute::<u16, TexPage>(
                    core::mem::transmute::<TexPage, u16>(level.textures[0].tex_page)
                        | ((Bpp::Bits8 as u16) << 7)
                        | 1 << 10,
                )
            };
            cube.set_tex_page(tex_page);
            if let Some(clut) = level.textures[0].clut {
                cube.set_clut(clut);
            }

            // Compute textured cube vertices
            const CUBE_SCALE: i16 = 80;
            let cube_vertices = [
                Vertex(-CUBE_SCALE / 2, -CUBE_SCALE / 2),
                Vertex(CUBE_SCALE / 2, -CUBE_SCALE / 2),
                Vertex(-CUBE_SCALE / 2, CUBE_SCALE / 2),
                Vertex(CUBE_SCALE / 2, CUBE_SCALE / 2),
            ];

            let (cos_x, sin_x) = (acos(cube_rot), asin(cube_rot));
            let cube_vertices = cube_vertices.map(|Vertex(vx, vy)| {
                Vertex(
                    (Fixed32::from_i16(vx) * cos_x - Fixed32::from_i16(vy) * sin_x + c_x).to_i16(),
                    (Fixed32::from_i16(vx) * sin_x + Fixed32::from_i16(vy) * cos_x + c_y).to_i16(),
                )
            });
            cube.set_color(Color {
                red: 255,
                green: 255,
                blue: 255,
            });
            cube.set_vertices(cube_vertices);
            cube.set_tex_coords([
                TexCoord { x: 0, y: 0 },
                TexCoord {
                    x: CUBE_SCALE as u8,
                    y: 0,
                },
                TexCoord {
                    x: 0,
                    y: CUBE_SCALE as u8,
                },
                TexCoord {
                    x: CUBE_SCALE as u8,
                    y: CUBE_SCALE as u8,
                },
            ]);
            add_prim::<PolyFT4>(&mut draw, cube, 3);

            // Draw text
            txt.link(&mut draw.otc[0]);
        });

        // Complete draw loop
        fb.draw_sync();
        fb.wait_vblank();
        fb.dma_swap(&mut gpu_dma);
    }
}

/// Update the sprite's transform
fn update_transform(transform: &mut SpriteTransform, velocity: &mut SpriteTransform, input: u16) {
    // Get angle in radians

    // Compute movement vector
    let angle = transform.rotation;
    let (mvt_x, mvt_y) = (asin(angle), acos(angle) * (-1));
    let (px, py) = &mut velocity.position;
    if input & PAD_UP == 0 {
        *px = *px + mvt_x * Fixed32(256);
        *py = *py + mvt_y * Fixed32(256);
    } else if input & PAD_DOWN == 0 {
        *px = *px - mvt_x * Fixed32(256);
        *py = *py - mvt_y * Fixed32(256);
    }

    // Add drag
    *px = *px * Fixed32(3950);
    *py = *py * Fixed32(3950);

    if input & PAD_LEFT == 0 {
        velocity.rotation = Fixed32((velocity.rotation - Fixed32(1024)).0.max((-360) << 12));
    }
    if input & PAD_RIGHT == 0 {
        velocity.rotation = Fixed32((velocity.rotation + Fixed32(1024)).0.min(360 << 12));
    }

    // Add drag
    velocity.rotation = velocity.rotation * Fixed32(3950);
}

/// Update the player's position and rotation from it's forces
fn update_physics(transform: &mut SpriteTransform, velocity: &mut SpriteTransform) {
    transform.position.0 =
        (transform.position.0 + velocity.position.0 + (RES_X as i32)) % (RES_X as i32);
    transform.position.1 =
        (transform.position.1 + velocity.position.1 + (RES_Y as i32)) % (RES_Y as i32);
    transform.rotation = (transform.rotation + velocity.rotation) % 360;
}

///////////////////
// Gamepad setup //
///////////////////

const PAD_BUF_SIZE: usize = 34;

// Gamepad consts
const PAD_UP: u16 = 0x10;
const PAD_RIGHT: u16 = 0x20;
const PAD_DOWN: u16 = 0x40;
const PAD_LEFT: u16 = 0x80;
const PAD_CROSS: u16 = 0x4000;
const PAD_SQUARE: u16 = 0x8000;

static mut PAD_BUFFER: [MaybeUninit<[u32; PAD_BUF_SIZE]>; 2] = [MaybeUninit::uninit(); 2];

// Initialize input listeners on the static buffer
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

///////////////////////
// Fixed32 structure //
///////////////////////

/// Structure for a fixed point value
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Fixed32(i32);
const FRACTION_SIZE: i32 = 12;

impl Fixed32 {
    fn from_i16(a: i16) -> Self {
        Self((a as i32) << FRACTION_SIZE)
    }

    fn to_i16(self) -> i16 {
        (self.0 >> FRACTION_SIZE) as i16
    }
}

impl Add<i32> for Fixed32 {
    type Output = Fixed32;
    fn add(mut self, rhs: i32) -> Self::Output {
        self.0 += rhs << FRACTION_SIZE;
        self
    }
}

impl Display for Fixed32 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{}.{:03}",
            self.0 >> FRACTION_SIZE,
            ((self.0 as u32) & (2_u32.pow(FRACTION_SIZE as u32) - 1)) * 1_000 / 4096
        )
    }
}

impl Add<Fixed32> for Fixed32 {
    type Output = Fixed32;
    fn add(self, rhs: Fixed32) -> Self::Output {
        Fixed32(self.0 + rhs.0)
    }
}

impl Sub<i32> for Fixed32 {
    type Output = Fixed32;
    fn sub(mut self, rhs: i32) -> Self::Output {
        self.0 -= rhs << 12;
        self
    }
}

impl Sub<Fixed32> for Fixed32 {
    type Output = Fixed32;
    fn sub(self, rhs: Fixed32) -> Self::Output {
        Fixed32(self.0 - rhs.0)
    }
}

impl Mul<i32> for Fixed32 {
    type Output = Fixed32;
    fn mul(self, rhs: i32) -> Self::Output {
        // Multiply decimal part
        Fixed32(rhs * self.0)
    }
}

impl Mul<Fixed32> for Fixed32 {
    type Output = Fixed32;
    fn mul(self, rhs: Fixed32) -> Self::Output {
        Fixed32((rhs.0 * self.0) >> FRACTION_SIZE)
    }
}

impl Div<i32> for Fixed32 {
    type Output = Fixed32;

    fn div(self, rhs: i32) -> Self::Output {
        Fixed32(self.0 / rhs)
    }
}

impl Div<Fixed32> for Fixed32 {
    type Output = Fixed32;

    fn div(self, rhs: Fixed32) -> Self::Output {
        Fixed32((self.0 << FRACTION_SIZE) / rhs.0) // At most keeps 7 bits of precision on decimal part
    }
}

impl Sum<Fixed32> for Fixed32 {
    fn sum<I: Iterator<Item = Fixed32>>(iter: I) -> Self {
        iter.reduce(|a, b| a + b).unwrap_or_default()
    }
}

impl Rem<i32> for Fixed32 {
    type Output = Fixed32;
    fn rem(self, rhs: i32) -> Self::Output {
        Fixed32(self.0 % (rhs << FRACTION_SIZE))
    }
}

impl Rem<Fixed32> for Fixed32 {
    type Output = Fixed32;
    fn rem(self, rhs: Fixed32) -> Self::Output {
        Fixed32(self.0 % rhs.0)
    }
}

impl Shr<i32> for Fixed32 {
    type Output = Fixed32;
    fn shr(self, rhs: i32) -> Self::Output {
        Fixed32(self.0 >> rhs)
    }
}

fn acos(x: Fixed32) -> Fixed32 {
    icos(x / 45)
}

fn asin(x: Fixed32) -> Fixed32 {
    isin(x / 45)
}

/// Taylor approximation to 3rd level of cos
/// Units: half-circles (i.e. PI / 2)
fn icos(x: Fixed32) -> Fixed32 {
    isin(x + Fixed32(1 << (FRACTION_SIZE + 1)))
}

/// 3rd degree approximation to 3rd level of sin
/// cf. https://www.coranac.com/2009/07/sines/
/// Units: half-circles (i.e. PI / 2)
fn isin(x: Fixed32) -> Fixed32 {
    let mut x = x.0;

    // Constants related to fraction point position and shift needed
    // for specific computations in the algorithm
    const QN: i32 = FRACTION_SIZE + 1;
    const QP: i32 = FRACTION_SIZE + 3;
    const QR: i32 = 2 * QN - QP;
    const QS: i32 = QN + QP + 1 - FRACTION_SIZE;

    unsafe {
        asm!(
            ".set noat",
            "sll {x}, {x}, {nshift}", // x := x << 30-13
            "sll $2, {x}, 1", // x_shifted := x << 1
            "xor $2, {x}, $2",

            // if x ^ (x<<1) > 0 {
            // x := (1 << 31) - x
            // }
            "bgez $2, 1f",
            "li $2, 1<<31",
            "subu {x}, $2, {x}",
            "1: sra {x}, {x}, {nshift}", // x >>= (30-13)

            // $2 := (3 << QP) - (x^2 >> QR)
            "mult {x}, {x}",
            "mflo $2",
            "li $3, {qpshift}",
            "sra $2, $2, {qr}",
            "sub $2, $3, $2",

            // x := (x * $2) >> QS
            "mult {x}, $2",
            "mflo {x}",
            "sra {x}, {x}, {qs}",
            out("$2") _,
            out("$3") _,
            nshift= const 30 - QN,
            qpshift = const 3 << QP,
            qr = const QR,
            qs = const QS,
            x = inout(reg) x
        );
    }
    Fixed32(x)
}

struct Rng;

impl Rng {
    fn new(seed: u32) -> Self {
        unsafe { kernel::psx_srand(seed) };
        Self
    }

    fn rand_range(&self, Range { start, end }: Range<u16>) -> u16 {
        (unsafe { kernel::psx_rand() }) % (end - start) + start
    }

    fn rand_range_u32(&self, Range { start, end }: Range<u32>) -> u32 {
        let num: u32 =
            ((unsafe { kernel::psx_rand() } as u32) << 16) + unsafe { kernel::psx_rand() } as u32;
        num % (end - start) + start
    }
}

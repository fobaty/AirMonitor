use display_interface_spi::SPIInterface;
use embedded_graphics::{
    mono_font::{ascii::*, MonoFont, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle, RoundedRectangle},
    text::Text,
};
use esp_idf_hal::{delay::Ets, gpio::PinDriver, spi::{config, SpiConfig, SpiDeviceDriver, SpiDriver}, units::*};
use mipidsi::{
    models::ST7735s,
    options::{ColorInversion, ColorOrder, Orientation, Rotation},
    Builder, Display,
};

use crate::config::GRAPH_SAMPLES;
use crate::sensors::{co2_level, pm_level};

pub type Tft<'d> = Display<
    SPIInterface<SpiDeviceDriver<'d, SpiDriver<'d>>, PinDriver<'d, esp_idf_hal::gpio::Gpio4, esp_idf_hal::gpio::Output>>,
    ST7735s,
    PinDriver<'d, esp_idf_hal::gpio::Gpio3, esp_idf_hal::gpio::Output>,
>;

pub const COLOR_BLACK: Rgb565 = Rgb565::BLACK;
pub const COLOR_WHITE: Rgb565 = Rgb565::WHITE;
pub const COLOR_GREEN: Rgb565 = Rgb565::new(0, 63, 0);
pub const COLOR_RED: Rgb565 = Rgb565::new(0, 0, 31);       // BGR corrected
pub const COLOR_CYAN: Rgb565 = Rgb565::new(31, 63, 0);     // BGR corrected
pub const COLOR_ORANGE: Rgb565 = Rgb565::new(0, 34, 31);   // BGR corrected
pub const COLOR_YELLOW: Rgb565 = Rgb565::new(0, 63, 31);
pub const COLOR_GRAY: Rgb565 = Rgb565::new(15, 59, 14);
const GRAPH_BORDER: Rgb565 = Rgb565::new(8, 32, 8);
const GRAPH_DOTS: Rgb565 = Rgb565::new(0, 31, 0);

pub const TOP_H: i32 = 32;
pub const BAR_H: i32 = 12;
pub const BAR_Y: i32 = 134;

type Dc<'d> = PinDriver<'d, esp_idf_hal::gpio::Gpio4, esp_idf_hal::gpio::Output>;
type Rst<'d> = PinDriver<'d, esp_idf_hal::gpio::Gpio3, esp_idf_hal::gpio::Output>;
type SpiT<'d> = SpiDeviceDriver<'d, SpiDriver<'d>>;

pub fn init_tft<'d>(
    driver: SpiDriver<'d>,
    cs_pin: esp_idf_hal::gpio::Gpio5,
    dc: Dc<'d>,
    rst: Rst<'d>,
    rotated: bool,
) -> Result<Tft<'d>, display_interface::DisplayError> {
    let spi: SpiT<'d> = SpiDeviceDriver::new(
        driver,
        Some(cs_pin),
        &SpiConfig::new()
            .baudrate(26_u32.MHz().into())
            .data_mode(config::MODE_0)
            .write_only(true)
            .input_delay_ns(0),
    )
    .map_err(|_| display_interface::DisplayError::BusWriteError)?;

    let di = SPIInterface::new(spi, dc);
    let mut delay = Ets;

    // Panel flipped: content is rotated 180° so it reads upright from the far edge.
    let orientation = if rotated {
        Orientation::new().rotate(Rotation::Deg180)
    } else {
        Orientation::new()
    };

    let display = Builder::new(ST7735s, di)
        .reset_pin(rst)
        .color_order(ColorOrder::Bgr)
        .invert_colors(ColorInversion::Normal)
        .orientation(orientation)
        .display_size(128, 160)
        .display_offset(2, 1)
        .init(&mut delay)
        .map_err(|_| display_interface::DisplayError::BusWriteError)?;

    Ok(display)
}

fn font(size: u32) -> &'static MonoFont<'static> {
    match size {
        2 => &FONT_9X15_BOLD,
        3 => &FONT_10X20,
        _ => &FONT_5X7,
    }
}

fn draw_text(
    display: &mut impl DrawTarget<Color = Rgb565>,
    s: &str,
    x: i32,
    y: i32,
    color: Rgb565,
    size: u32,
) {
    let f = font(size);
    let mut style = MonoTextStyle::new(f, color);
    style.background_color = Some(COLOR_BLACK);
    // embedded-graphics places the glyph box `baseline` rows above position.y
    Text::new(s, Point::new(x, y + f.baseline as i32), style)
        .draw(display)
        .ok();
}

pub fn fill_screen(display: &mut impl DrawTarget<Color = Rgb565>) {
    display.clear(COLOR_BLACK).ok();
}

pub fn clear_rect(display: &mut impl DrawTarget<Color = Rgb565>, x: i32, y: i32, w: i32, h: i32) {
    Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyle::with_fill(COLOR_BLACK))
        .draw(display)
        .ok();
}

// ─── Splash screen (C++ setup: border + AIR SCAN + VERSION + SYSTEM CHECK) ───

pub fn draw_splash_border(display: &mut Tft, version: &str) {
    fill_screen(display);
    RoundedRectangle::with_equal_corners(
        Rectangle::new(Point::new(5, 5), Size::new(118, 40)),
        Size::new(8, 8),
    )
    .into_styled(PrimitiveStyle::with_stroke(COLOR_CYAN, 2))
    .draw(display)
    .ok();

    draw_text(display, "AIR SCAN", 20, 10, COLOR_CYAN, 2);
    draw_text(display, version, 42, 27, COLOR_WHITE, 1);
    draw_text(display, "SYSTEM CHECK:", 10, 52, COLOR_WHITE, 1);
}

pub fn splash_check(display: &mut Tft, y: i32, label: &str, ok: bool) -> i32 {
    draw_text(display, &format!(" > {}:", label), 10, y, COLOR_WHITE, 1);
    let color = if ok { COLOR_GREEN } else { COLOR_RED };
    draw_text(display, if ok { "OK" } else { "FAIL" }, 95, y, color, 1);
    y + 7
}

pub fn splash_text(display: &mut Tft, s: &str, y: i32) -> i32 {
    draw_text(display, s, 10, y, COLOR_WHITE, 1);
    y + 7
}

pub fn wifi_attempt(display: &mut Tft, y: i32, idx: u32, ssid: &str, dots: usize) {
    clear_rect(display, 0, y, 128, 7);
    let mut s = format!("Trying [{}]: {}", idx, ssid);
    for _ in 0..dots {
        s.push('.');
    }
    draw_text(display, &s, 10, y, COLOR_WHITE, 1);
}

pub fn wifi_message(display: &mut Tft, y: i32, s: &str, color: Rgb565) {
    clear_rect(display, 0, y, 128, 7);
    draw_text(display, s, 10, y, color, 1);
}

// ─── loop step 1: UI progress bar ───

pub fn draw_progress_bar(display: &mut Tft, bw: i32, last_bw: &mut i32) {
    let bw = bw.clamp(0, 128);
    if bw < *last_bw {
        clear_rect(display, 0, 158, 128, 2);
    }
    if bw > 0 {
        Rectangle::new(Point::new(0, 158), Size::new(bw as u32, 2))
            .into_styled(PrimitiveStyle::with_fill(COLOR_CYAN))
            .draw(display)
            .ok();
    }
    *last_bw = bw;
}

// ─── loop step 5: UI clock (top 32px) ───

const GLYPH_W: usize = 5;

fn glyph_rows(ch: char) -> &'static [u8; 7] {
    match ch {
        '0' => &[0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => &[0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => &[0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        '3' => &[0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => &[0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => &[0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => &[0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => &[0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => &[0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => &[0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        ':' => &[0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
        'A' => &[0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'M' => &[0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'P' => &[0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        _ => &[0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
    }
}

fn draw_glyph(
    display: &mut impl DrawTarget<Color = Rgb565>,
    rows: &[u8; 7],
    x: i32,
    y: i32,
    scale: i32,
    color: Rgb565,
) {
    for (r, row) in rows.iter().enumerate() {
        for c in 0..GLYPH_W {
            if row & (0b10000 >> c) != 0 {
                Rectangle::new(
                    Point::new(x + c as i32 * scale, y + r as i32 * scale),
                    Size::new(scale as u32, scale as u32),
                )
                .into_styled(PrimitiveStyle::with_fill(color))
                .draw(display)
                .ok();
            }
        }
    }
}

pub fn draw_clock(
    display: &mut Tft,
    has_time: bool,
    h: u32,
    m: u32,
    s: u32,
    uptime_s: u64,
) {
    clear_rect(display, 0, 0, 128, TOP_H);

    if has_time {
        let h12 = if h % 12 == 0 { 12 } else { h % 12 };
        let sep = if s % 2 == 0 { ':' } else { ' ' };
        let tstr = format!("{:02}{}{:02}", h12, sep, m);
        let mut gx = 6;
        for ch in tstr.chars() {
            draw_glyph(display, glyph_rows(ch), gx, 4, 3, COLOR_CYAN);
            gx += GLYPH_W as i32 * 3 + 2;
        }
        let pstr = if h < 12 { "AM" } else { "PM" };
        let mut px = 96;
        for ch in pstr.chars() {
            draw_glyph(display, glyph_rows(ch), px, 5, 1, COLOR_WHITE);
            px += GLYPH_W as i32 + 1;
        }
    } else {
        let total_h = (uptime_s / 3600) % 100;
        let mins = (uptime_s % 3600) / 60;
        draw_text(display, "UPTIME", 10, 5, COLOR_WHITE, 1);
        draw_text(display, &format!("{:02}:{:02}", total_h, mins), 10, 13, COLOR_WHITE, 2);
        let dot_on = s % 2 == 0;
        Circle::new(Point::new(120, 11), 2)
            .into_styled(PrimitiveStyle::with_fill(if dot_on { COLOR_RED } else { COLOR_BLACK }))
            .draw(display)
            .ok();
    }
}

// Blink the clock colon in place without redrawing the digits (avoids flicker).
pub fn clock_colon_blink(display: &mut Tft, show: bool) {
    let colon_x = 6 + (GLYPH_W as i32 * 3 + 2) * 2;
    clear_rect(display, colon_x, 4, 17, 21);
    if show {
        draw_glyph(display, glyph_rows(':'), colon_x, 4, 3, COLOR_CYAN);
    }
}

// Blink the uptime LED dot in place.
pub fn clock_uptime_led(display: &mut Tft, on: bool) {
    Circle::new(Point::new(120, 11), 2)
        .into_styled(PrimitiveStyle::with_fill(if on { COLOR_RED } else { COLOR_BLACK }))
        .draw(display)
        .ok();
}

// ─── loop step 6: UI refresh data (every 5s) ───

fn level_rgb(lvl: &str) -> Rgb565 {
    match lvl {
        "Good" => COLOR_GREEN,
        "Fair" => COLOR_ORANGE,
        _ => COLOR_RED,
    }
}

pub fn draw_data(
    display: &mut Tft,
    co2: f32,
    temp: f32,
    hum: f32,
    pm1: u16,
    pm25: u16,
    pm10: u16,
    m_en: bool,
    m_con: bool,
    wifi_connected: bool,
    ip: &str,
    co2_hist: &[i32; GRAPH_SAMPLES],
) {
    let co2_lvl = co2_level(co2);
    draw_text(
        display,
        &format!("CO2: {:.0} ({})", co2, co2_lvl),
        1, 32, level_rgb(co2_lvl), 1,
    );

    draw_co2_graph(display, 4, 42, 120, 28, co2, co2_hist);

    draw_text(display, &format!("T:{:.1}C H:{:.0}%", temp, hum), 1, 73, COLOR_WHITE, 1);

    for (y, (lbl, v)) in [(87, ("PM1.0", pm1)), (101, ("PM2.5", pm25)), (115, ("PM10", pm10))] {
        let lvl = pm_level(v);
        let color = level_rgb(lvl);
        draw_text(display, &format!("{}: {}", lbl, v), 1, y, color, 1);
        draw_text(display, lvl, 70, y, color, 1);
    }

    clear_rect(display, 0, BAR_Y, 128, BAR_H);
    let (ms, mc) = if !m_en {
        ("M:OFF", COLOR_CYAN)
    } else if m_con {
        ("M:OK", COLOR_GREEN)
    } else {
        ("M:ERR", COLOR_RED)
    };
    draw_text(display, ms, 1, BAR_Y, mc, 1);
    let ip_color = if wifi_connected { COLOR_GRAY } else { COLOR_YELLOW };
    draw_text(display, &format!(" {}", ip), 26, BAR_Y, ip_color, 1);
}

pub fn draw_co2_graph(
    display: &mut impl DrawTarget<Color = Rgb565>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    co2: f32,
    history: &[i32; GRAPH_SAMPLES],
) {
    clear_rect(display, x, y, w, h);

    Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyle::with_stroke(GRAPH_BORDER, 1))
        .draw(display)
        .ok();

    let clamped_co2 = co2.clamp(400.0, 2000.0);
    let nl = (((clamped_co2 - 400.0) / 1600.0) * (h - 4) as f32) as i32;

    let mut i = 0;
    while i < w {
        let px = x + i;
        let py = y + h - 2 - nl;
        Circle::new(Point::new(px, py), 1)
            .into_styled(PrimitiveStyle::with_fill(GRAPH_DOTS))
            .draw(display)
            .ok();
        i += 4;
    }

    for idx in 0..(GRAPH_SAMPLES - 1) {
        if history[idx + 1] == 0 {
            break;
        }
        let v1 = (history[idx].clamp(400, 2000) - 400) as f32 / 1600.0 * (h - 4) as f32;
        let v2 = (history[idx + 1].clamp(400, 2000) - 400) as f32 / 1600.0 * (h - 4) as f32;

        let x1 = x + (idx as i32) * 2;
        let y1 = y + h - 2 - v1 as i32;
        let x2 = x + ((idx + 1) as i32) * 2;
        let y2 = y + h - 2 - v2 as i32;

        Line::new(Point::new(x1, y1), Point::new(x2, y2))
            .into_styled(PrimitiveStyle::with_stroke(COLOR_GREEN, 1))
            .draw(display)
            .ok();
    }
}
use esp_idf_hal::i2c::I2cDriver;
use esp_idf_hal::uart::UartDriver;
use log::{error, info};

pub fn pm_level(v: u16) -> &'static str {
    if v <= 12 { "Good" }
    else if v <= 35 { "Fair" }
    else if v <= 55 { "Poor" }
    else { "Bad" }
}

pub fn co2_level(v: f32) -> &'static str {
    if v <= 800.0 { "Good" }
    else if v <= 1200.0 { "Fair" }
    else if v <= 2000.0 { "Poor" }
    else { "Bad" }
}

pub fn level_color(lvl: &str) -> &'static str {
    match lvl {
        "Good" => "green",
        "Fair" => "orange",
        "Poor" | "Bad" => "red",
        _ => "darkred",
    }
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0xFFu8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x31;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

pub struct Scd30Sensor<'d> {
    i2c: I2cDriver<'d>,
}

impl<'d> Scd30Sensor<'d> {
    pub fn new(i2c: I2cDriver<'d>) -> Self {
        Self { i2c }
    }

    pub fn init(&mut self) -> bool {
        // Set measurement interval to 2 s (0x4600 + CRC) then start periodic measurement (0x0010).
        let interval = [0x46, 0x00, 0x00, 0x02, crc8(&[0x00, 0x02])];
        let start = [0x00, 0x10];
        if self.i2c.write(0x61, &interval, 1000).is_err() {
            error!("SCD30 interval cmd failed");
            return false;
        }
        match self.i2c.write(0x61, &start, 1000) {
            Ok(_) => {
                info!("SCD30 initialized");
                true
            }
            Err(e) => {
                error!("SCD30 init failed: {:?}", e);
                false
            }
        }
    }

    pub fn raw_data_ready(&mut self) -> Option<u16> {
        let cmd = [0x02, 0x02];
        let mut buf = [0u8; 3];
        match self.i2c.write_read(0x61, &cmd, &mut buf, 1000) {
            Ok(_) => Some((buf[0] as u16) << 8 | (buf[1] as u16)),
            Err(e) => {
                error!("SCD30 data_ready I2C error: {:?}", e);
                None
            }
        }
    }

    pub fn data_ready(&mut self) -> bool {
        self.raw_data_ready() == Some(1)
    }

    pub fn read_u16(&mut self, reg: u16) -> Option<(u16, bool)> {
        let cmd = reg.to_be_bytes();
        let mut buf = [0u8; 3];
        self.i2c.write_read(0x61, &cmd, &mut buf, 1000).ok()?;
        let v = (buf[0] as u16) << 8 | (buf[1] as u16);
        let crc_ok = crc8(&buf[0..2]) == buf[2];
        Some((v, crc_ok))
    }

    pub fn read_measurement_raw(&mut self) -> Option<[u8; 18]> {
        let cmd = [0x03, 0x00];
        let mut buf = [0u8; 18];
        self.i2c.write_read(0x61, &cmd, &mut buf, 1000).ok()?;
        Some(buf)
    }

    pub fn read_measurement(&mut self) -> Result<(f32, f32, f32), &'static str> {
        let cmd = [0x03, 0x00];
        let mut buf = [0u8; 18];
        self.i2c.write_read(0x61, &cmd, &mut buf, 1000).map_err(|_| "I2C read error")?;

        fn decode_float(b: &[u8]) -> f32 {
            let u = u32::from_be_bytes([b[0], b[1], b[3], b[4]]);
            f32::from_bits(u)
        }

        let co2 = decode_float(&buf[0..6]);
        let temp = decode_float(&buf[6..12]);
        let hum = decode_float(&buf[12..18]);
        Ok((co2, temp, hum))
    }
}

pub struct PmsSensor<'d> {
    uart: UartDriver<'d>,
}

impl<'d> PmsSensor<'d> {
    pub fn new(uart: UartDriver<'d>) -> Self {
        Self { uart }
    }

    pub fn read_data(&mut self, pm1: &mut u16, pm25: &mut u16, pm10: &mut u16) {
        let mut data = [0u8; 64];
        match self.uart.read(&mut data, 10) {
            Ok(n) if n > 0 => {
                let mut i = 0;
                while i + 31 < n {
                    if data[i] == 0x42 && data[i + 1] == 0x4D {
                        let buf = &data[i + 2..i + 32];
                        *pm1 = (buf[8] as u16) << 8 | (buf[9] as u16);
                        *pm25 = (buf[10] as u16) << 8 | (buf[11] as u16);
                        *pm10 = (buf[12] as u16) << 8 | (buf[13] as u16);
                        break;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
    }
}

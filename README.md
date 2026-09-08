# 🌬️ AirMonitor — ESP32-S3 Air Quality Monitor (Rust)

A compact indoor air quality monitor built on an **ESP32-S3** and written in
**Rust** (`esp-idf-svc` / `embedded-graphics`). It measures CO₂, temperature,
humidity and particulate matter, shows them on a small **ST7735** TFT and
publishes the readings over Wi-Fi — without cloud dependencies.

```
┌─────────────────────────────┐
│   AirMonitor V2.1            │
│   ┌───────────────────────┐  │
│   │  CO2: 902 (Good)      │  │  ← live CO₂ + level
│   │  ╭─────────────────╮  │  │  ← scrolling CO₂ graph (30s/pt)
│   │  ╰─────────────────╯  │  │
│   │  T:29.8C H:45%        │  │
│   │  PM1.0 Good    1      │  │
│   │  PM2.5 Good    1      │  │
│   │  PM10  Good    1      │  │
│   │  M:OFF  10.32.205.217 │  │  ← MQTT + IP
│   └───────────────────────┘  │
└─────────────────────────────┘
```

## Features

- 🧪 **CO₂ / temperature / humidity** — Sensirion **SCD30** over I²C
- 🌫️ **PM1.0 / PM2.5 / PM10** — **PMS5003** over UART
- 🖥️ **128×160 ST7735** display with `embedded-graphics` + `mipidsi`
  - live CO₂ reading with an AQI-style color level (`Good` / `Fair` / `Poor` / `Bad`)
  - scrolling **CO₂ history graph**, one point every 30 s
  - large 3×-scaled clock, synced via **NTP** (SNTP)
  - **180° rotation** toggled from the web UI, persisted in NVS (default: flipped)
- 📶 **Wi-Fi**: connects to previously saved networks; falls back to a
  **configurable AP** with an embedded settings page
- 🎛️ **Web configuration** (STA mode): scan networks, connect, erase saved list,
  rotate display
- 📤 **MQTT**: optional publish of all readings to a broker
- 🔌 Boot-level debug via the integrated **USB-Serial-JTAG** console

## Hardware & Pinout

| Component | Pins |
|---|---|
| TFT (ST7735, SPI2) | SCK 1 · MOSI 2 · RST 3 · DC 4 · CS 5 · BL 6 |
| SCD30 (I²C0) | SDA 12 · SCL 13 |
| PMS5003 (UART1) | RX 11 · TX 10 |

All pins are defined in [`src/config.rs`](src/config.rs).

## Getting started

### 1. Toolchain

Requires the Rust **ESP** toolchain and `espflash`:

```bash
rustup toolchain install esp --profile esp
cargo install espup ldproxy espflash
espup install          # installs Xtensa tools + export-esp.sh
source "$HOME/export-esp.sh"
```

Or just run the bundled [`flash.sh`](flash.sh) — it installs everything it needs
and builds + flashes + opens the monitor.

### 2. Build & flash

```bash
cargo +esp check
cargo +esp build --release
espflash flash --monitor target/xtensa-esp32s3-espidf/release/air-monitor-rust
```

### 3. First boot & Wi-Fi setup

1. The device tries the saved Wi-Fi networks. If none succeed it starts an
   access point: **`AIR-SCAN-CONFIG`** / password **`12345678`**.
2. Connect to the AP and open **`http://192.168.71.1`**.
3. Use **📡 Scan WiFi** → pick a network → enter password → **Save**.
   The device stores up to 5 networks in NVS and connects to the best one on boot.

## Web interface

| Route | Method | Purpose |
|---|---|---|
| `/` | GET | configuration UI (serve from `src/index.html`) |
| `/status` | GET | current sensor readings as JSON |
| `/scan` | GET | scan & list available Wi-Fi networks |
| `/connect` | POST | save a network and reconnect |
| `/clearwifi` | GET | erase all saved networks |
| `/rotate` | GET | toggle display 180° and reboot |

### `/status` example

```json
{
  "co2": 902, "co2lvl": "Good", "co2clr": "green",
  "temp": 29.8, "hum": 45,
  "pm1": 1, "pm25": 1, "pm10": 1,
  "mqtt_en": 0, "qtty": 0, "rot": 1
}
```

## MQTT

MQTT is optional and configured through the web UI (broker, user, password,
port). When enabled, the device publishes JSON every 10 s to topic
**`air/status`**. All settings live in NVS.

## Project layout

```
src/
├── main.rs     # app wiring: TFT, sensors, Wi-Fi, web server, MQTT, NTP
├── display.rs  # ST7735 init, layout, text/graph/clock drawing
├── sensors.rs  # SCD30 (I²C) + PMS5003 (UART) drivers with Sensirion CRC
├── network.rs  # NVS store, Wi-Fi steering, AP fallback
├── config.rs   # pin map, AP credentials, intervals, MQTT topic
└── index.html  # embedded web UI (embedded via include_str!)
```

## Troubleshooting

- **SCD30 returns zeros / `raw_ready=0`** — the module must be read with
  *separate* write and read transactions (Arduino `Wire` semantics). Combined
  `write_read` (repeated START) produced all-zero frames on real hardware; the
  firmware deliberately uses `write` → `read`. See `Scd30Sensor::read_cmd`.
- **No serial output after flashing** — toggle DTR in your terminal or reset
  the board; the USB-Serial-JTAG console needs a reset pulse.
- **Port name changes** — watch for `/dev/cu.usbmodem*` on macOS/Linux.

## License

[MIT](LICENSE)
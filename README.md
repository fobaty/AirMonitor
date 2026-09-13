# 🌬️ AirMonitor — ESP32-S3 Air Quality Monitor (Rust)

A compact indoor air quality monitor built on an **ESP32-S3** and written in
**Rust** (`esp-idf-svc` / `embedded-graphics`). It measures CO₂, temperature,
humidity and particulate matter, shows them on a small **ST7735** TFT and
publishes the readings over Wi-Fi — without any cloud dependencies.

```
┌─────────────────────────────┐
│   AirMonitor V0.1.20        │
│   ┌───────────────────────┐ │
│   │ CO2: 902 (Good)       │ │  ← live CO₂ + AQI level
│   │ ╭─2k ppm────────────╮ │ │  ← CO₂ graph box (120x28 px)
│   │ │ ╭─▄▇█▇▆▄▂─────────│ │ │    - 60 samples (1 / 30s)
│   │ │ ╰···400 ppm·······│ │ │    - clamped 400-2000 ppm range
│   │ ╰───────────────────╯ │ │    - green polyline + baseline dots
│   │ T:29.8C H:45%         │ │  ← temperature & humidity
│   │ PM1.0 Good    5       │ │
│   │ PM2.5 Good    7       │ │
│   │ PM10  Good    5       │ │
│   │ M:OK   192.168.1.178  │ │  ← MQTT state + IP
│   └───────────────────────┘ │
└─────────────────────────────┘
```

### CO₂ History Graph Details (`display.rs`)

- **Box Dimensions:** Rendered within a boxed viewport of `120 × 28` pixels (`x=4, y=44`), outlined with `GRAPH_BORDER`.
- **Rolling Time Window:** Maintains a ring buffer of **60 sample points** (`co2_hist[60]`), with a new sample pushed every **30 seconds** (total history window: **30 minutes**).
- **Value Clamping & Scaling:**
  - Readings are clamped between **400 ppm** (graph bottom baseline) and **2000 ppm** (graph top boundary).
  - Linear vertical mapping: `((clamped - 400) / 1600) * (height - 4)`.
- **Rendering Elements:**
  - **Baseline Dots:** Background dot markers (`·`) plotted horizontally every 4 pixels across the graph base.
  - **Trend Polyline:** Connected line segments (`Line::new`) rendered in bright green (`COLOR_GREEN`) linking consecutive sample points.
  - **Zero-Sentinel Protection:** Uninitialized or empty history slots (`0`) act as sentinels, stopping the polyline rendering so only valid recorded data is displayed.

## Features

- 🧪 **CO₂ / temperature / humidity** — Sensirion **SCD30** over I²C
  (separate write/read transactions; see Troubleshooting)
- 🌫️ **PM1.0 / PM2.5 / PM10** — **PMS5003** over UART
- 🖥️ **128×160 ST7735** display, `embedded-graphics` + `mipidsi`
  - live CO₂ with colour level (`Good` / `Fair` / `Poor` / `Bad`)
  - scrolling CO₂ **history graph** (60 points, one per 30 s)
  - large clock synced via **NTP (SNTP)**
  - configurable **night dimming**: window (start/end hour) **plus a
    brightness level (1–1024)** — persist as dim as you like
  - **180° rotation**, toggled from the web UI, persisted in NVS
- 📶 **Wi-Fi**: connects to a saved network; boots in `AP + STA` so both work
  during the first minute, then drops the AP once STA is stable for 60 s.
  Falls back to a **configurable AP** (`AIR-SCAN-CONFIG`) whenever the home
  network is lost, with a live settings page inside.
- 🎛️ **Web interface** (embedded in firmware):
  - **dark theme**, centered layout, two tabs: Dashboard + Settings
  - scan & join Wi-Fi, MQTT broker, timezone/DST, night mode, maintainence
  - shows the **saved SSID / broker host / port / user** (passwords are never
    sent back or stored in plain view)
  - firmware version shown on the page
- 📤 **MQTT**: optional publish every 10 s to topic **`air/status`**
  (broker at `192.168.1.99:1883` in the example) with detailed connection
  logs, incl. error reasons when a broker is unreachable
- 🚀 **OTA firmware update** over Wi-Fi — no USB cable needed
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

Or just run the bundled [`flash.sh`](flash.sh) — it installs everything it
needs, then builds + flashes (bootloader + partition table) and opens the
monitor.

### 2. Build & flash

The project uses **Two-OTA partitions** (`partitions.csv`) so updates can also
be delivered over the air. For a serial flash always pass the bootloader and
partition table:

```bash
cargo +esp check
cargo +esp build --release
espflash flash --baud 921600 \
  --bootloader target/xtensa-esp32s3-espidf/release/bootloader.bin \
  --partition-table partitions.csv \
  target/xtensa-esp32s3-espidf/release/air-monitor-rust
```

### 3. First boot & Wi-Fi setup

1. The device tries its saved Wi-Fi networks; if none connect it starts an AP:
   **`AIR-SCAN-CONFIG`** / password **`12345678`** at `192.168.71.1`.
2. Connect to the AP and open **`http://192.168.71.1`**.
3. Use **📡 Scan WiFi** → pick a network → enter password → **Save & Restart**.
   The device stores the network in NVS and reconnects on boot.

> The settings form shows the currently saved values (SSID, broker host, port,
> user, night brightness). Leaving a password field blank **keeps** the saved
> one — it is never erased by an empty form submit.

## Web interface

| Route | Method | Purpose |
|---|---|---|
| `/` | GET | UI (dark theme, Dashboard + Settings) — served from `src/index.html` |
| `/status` | GET | current readings + saved config as JSON |
| `/scan` | GET | scan & list nearby Wi‑Fi networks |
| `/connect` | POST | save network / MQTT / timezone / night settings, then restart |
| `/clearwifi` | GET | erase all saved networks |
| `/rotate` | GET | toggle display 180° and reboot |
| `/logs` | GET | recent boot / runtime log lines (ring buffer) |
| `/ota` | GET / POST | firmware update via upload (OTA) |

### `/status` example

```json
{
  "co2": 902, "co2lvl": "Good", "co2clr": "green",
  "pm1": 1, "pm25": 1, "pm10": 1,
  "temp": 29.8, "hum": 45,
  "mq": "OFF", "m_en": false,
  "gmt_h": 2, "dst_s": 0,
  "ssid": "MyNetwork", "ip": "192.168.1.178",
  "rot": 1, "n_on": 0, "n_sh": 23, "n_eh": 7, "n_lev": 40,
  "m_srv": "192.168.1.99", "m_port": 1883, "m_user": "esp32air",
  "ver": "V0.1.17"
}
```

> **Security note:** the JSON never includes passwords (`wpass` / `m_pass`);
> only non-secret settings are echoed back to the UI.

## MQTT

MQTT is optional and configured through the web UI (broker host, port, user,
password). When enabled it publishes every **10 s** to topic **`air/status`**
as JSON, adding a `t` timestamp. Connection progress and errors appear in
`/logs` (`MQTT connecting to …`, `MQTT connected to …`, `MQTT connection
error: …`). All settings live in NVS.

## OTA updates

Releases are built by GitHub Actions on a pushed `v*` tag:

```bash
git tag v0.1.17 && git push origin v0.1.17
# → GitHub Action builds → publishes air-monitor-esp32s3-v0.1.17.bin
```

1. Open the web UI and go to **Maintenance → Firmware OTA**.
2. Select the `.bin`, upload it; the device flashes the inactive OTA slot and
   reboots automatically.
3. Confirm the running version on the Dashboard (`Firmware V0.1.17`).

## Project layout

```
src/
├── main.rs     # app wiring: TFT, sensors, Wi-Fi, web server, MQTT, NTP, OTA
├── display.rs  # ST7735 init, layout, text/graph/clock drawing
├── sensors.rs  # SCD30 (I²C) + PMS5003 (UART) drivers with Sensirion CRC
├── network.rs  # NVS store, Wi-Fi steering, MQTT config
├── config.rs   # pin map, AP credentials, intervals, night defaults, version
└── index.html  # embedded web UI (via include_str!)
partitions.csv # Two-OTA partition table (bootloader, otadata, ota_0, ota_1)
```

## Troubleshooting

- **SCD30 returns zeros / `raw_ready=0`** — the module must be read with
  *separate* write and read transactions (Arduino `Wire` semantics). Combined
  `write_read` (repeated START) produced all-zero frames on real hardware; the
  firmware deliberately uses `write` → `read`. See `Scd30Sensor::read_cmd`.
- **No serial output after flashing** — the USB-Serial-JTAG console needs a
  reset pulse: toggle **DTR** in your terminal tool or press the board reset.
- **Port name changes / `Port busy`** — watch for `/dev/cu.usbmodem*` on
  macOS/Linux; kill stale `espflash` (`lsof /dev/cu.usbmodem*`) before
  re-flashing.
- **Wi‑Fi keeps dropping after ~1 min** — older builds tore down the AP with a
  full Wi‑Fi stack restart (`w.stop()`), dropping STA too. Current firmware
  switches modes in place, so the STA link stays up; a lingering home-router
  "association refused" usually points to a full/limited AP. Check the router's
  client limit and the device's `/logs`.
- **Lots of `EspNvs dropped` log spam** — NVS is now read once at boot and
  cached; the log is quiet. If you see it endlessly, the firmware stored
  config is still being re-read (older build).
- **MQTT shows FAIL** — check `/logs` for the exact reason (`server host is
  empty`, port unreachable, auth error).

## License

[MIT](LICENSE)
#!/usr/bin/env bash
set -e

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_DIR"

echo "=== AirMonitor ESP32-S3 Build & Flash Tool ==="

# Source esp-rs environment if available
if [ -f "$HOME/export-esp.sh" ]; then
    source "$HOME/export-esp.sh"
fi

# 1. Check and install Rust ESP toolchain if missing
if ! rustup toolchain list | grep -q "esp"; then
    echo "[-] Rust ESP toolchain not detected."
    if ! command -v espup &> /dev/null; then
        echo "[*] Installing espup..."
        cargo install espup
    fi
    echo "[*] Installing ESP toolchain via espup (this may take a few minutes)..."
    espup install
    
    if [ -f "$HOME/export-esp.sh" ]; then
        source "$HOME/export-esp.sh"
    fi
fi

# 2. Check and install ldproxy if missing
if ! command -v ldproxy &> /dev/null; then
    echo "[*] Installing ldproxy..."
    cargo install ldproxy
else
    echo "[+] ldproxy is installed."
fi

# 3. Check and install espflash if missing
if ! command -v espflash &> /dev/null; then
    echo "[*] Installing espflash..."
    cargo install espflash
else
    echo "[+] espflash is installed."
fi

# 4. Run cargo +esp check
echo "[*] Running cargo +esp check..."
cargo +esp check

# 5. Build and Flash using espflash directly
echo "[*] Building and flashing to ESP32..."
cargo +esp build --release
espflash flash --monitor target/xtensa-esp32s3-espidf/release/air-monitor-rust

echo "[+] Done!"

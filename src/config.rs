pub const VERSION: &str = "V0.1.17";
pub const AP_SSID_DEF: &str = "AIR-SCAN-CONFIG";
pub const AP_PASS_DEF: &str = "12345678";
pub const WIFI_TIMEOUT_MS: u64 = 10000;
pub const MAX_WIFI_NETWORKS: usize = 5;

pub const I2C_SDA_PIN: u32 = 12;
pub const I2C_SCL_PIN: u32 = 13;
pub const PMS_RX_PIN: u32 = 11;
pub const PMS_TX_PIN: u32 = 10;
pub const TFT_SCK_PIN: u32 = 1;
pub const TFT_MOSI_PIN: u32 = 2;
pub const TFT_RST_PIN: u32 = 3;
pub const TFT_DC_PIN: u32 = 4;
pub const TFT_CS_PIN: u32 = 5;
pub const TFT_BL_PIN: u32 = 6;

// Backlight PWM (LEDC, 10-bit resolution => max duty 1024). Night mode dims the
// panel so it does not glare in a dark room; defaults are a 23:00-07:00 window.
pub const BL_FULL_DUTY: u32 = 1024;
pub const NIGHT_DIM_LEVEL: u32 = 40;
pub const NIGHT_ON_DEF: bool = false;
pub const NIGHT_START_H_DEF: i32 = 23;
pub const NIGHT_END_H_DEF: i32 = 7;

pub const GRAPH_SAMPLES: usize = 60;
pub const GRAPH_INTERVAL_MS: u64 = 30000;
pub const DISPLAY_UPDATE_INTERVAL_MS: u64 = 5000;
pub const MQTT_INTERVAL_MS: u64 = 10000;
pub const MQTT_RETRY_INTERVAL_MS: u64 = 15000;
pub const MQTT_TOPIC: &str = "air/status";

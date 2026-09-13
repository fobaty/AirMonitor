use esp_idf_svc::hal::prelude::*;
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::uart::{UartConfig, UartDriver};
use esp_idf_svc::hal::gpio::*;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::wifi::{AccessPointConfiguration, AuthMethod, ClientConfiguration, Configuration, EspWifi};
use esp_idf_svc::http::server::{EspHttpServer, Configuration as HttpConf};
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration};
use embedded_svc::io::Write;
use embedded_svc::http::Headers;
use embedded_svc::mqtt::client::{EventPayload, QoS};

use log::*;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod config;
mod sensors;
mod display;
mod network;
mod logbuf;

struct AppData {
    co2: f32,
    temperature: f32,
    humidity: f32,
    pm1: u16,
    pm25: u16,
    pm10: u16,
    mqtt_enabled: bool,
    mqtt_connected: bool,
    wifi_ssid: String,
    wifi_ip: String,
    gmt_off: i32,
    dst_off: i32,
    night_on: bool,
    night_sh: i32,
    night_eh: i32,
    night_lev: u32,
    ap_up: bool,
}

/// Boot-time snapshot of NVS configuration. Read once at startup because NVS is
/// only written via /connect, after which the device restarts. Keeping this in
/// RAM avoids reopening the NVS partition on every /status poll.
struct Cfg {
    rotated: bool,
    mqtt: network::MqttCfg,
    wifi_ssid: String,
    night_on: bool,
    night_sh: i32,
    night_eh: i32,
    night_lev: u32,
}

fn url_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s.replace('+', " ").as_str())
        .decode_utf8_lossy()
        .into_owned()
}

fn parse_form(body: &str) -> std::collections::HashMap<String, String> {
    let mut params = std::collections::HashMap::new();
    for pair in body.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            params.insert(url_decode(k), url_decode(v));
        } else if !pair.is_empty() {
            params.insert(url_decode(pair), String::new());
        }
    }
    params
}

// True when local hour h falls inside the [sh, eh) night window. sh == eh means
// "always night" (useful to test dimming), otherwise wrap-around is supported
// (e.g. 23:00 -> 07:00).
fn night_dim(h: u32, sh: i32, eh: i32) -> bool {
    let h = h as i32;
    if sh == eh {
        true
    } else if sh < eh {
        sh <= h && h < eh
    } else {
        h >= sh || h < eh
    }
}

fn ap_cfg() -> AccessPointConfiguration {
    AccessPointConfiguration {
        ssid: heapless::String::<32>::try_from(config::AP_SSID_DEF).unwrap(),
        password: heapless::String::<64>::try_from(config::AP_PASS_DEF).unwrap(),
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }
}

// Local time (C++ getLocalTime): SystemTime + gmt/dst offsets, validated by year.
fn local_hms(gmt_off: i32, dst_off: i32) -> (bool, u32, u32, u32) {
    let now_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if now_s == 0 {
        return (false, 0, 0, 0);
    }
    let shifted = now_s + gmt_off as i64 + dst_off as i64;
    let day = shifted.div_euclid(86400);
    let year = 1970 + day / 365;
    let sod = shifted.rem_euclid(86400);
    if year < 2020 {
        return (false, 0, 0, 0);
    }
    (
        true,
        (sod / 3600) as u32,
        ((sod % 3600) / 60) as u32,
        (sod % 60) as u32,
    )
}

fn main() {
    esp_idf_svc::sys::link_patches();
    logbuf::init();
    info!("AirMonitor {} starting", config::VERSION);

    let peripherals = Peripherals::take().unwrap();
    let sys_loop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    let store = network::NvsStore::new(nvs.clone());
    let mqtt_cfg = store.load_mqtt();
    let stored = store.get_wifi_list();
    let (night_on, night_sh, night_eh, night_lev) = store.get_night();
    let rotated = store.get_display_rot();
    let cfg = Arc::new(Cfg {
        rotated,
        mqtt: mqtt_cfg.clone(),
        wifi_ssid: stored.first().map(|(s, _)| s.clone()).unwrap_or_default(),
        night_on,
        night_sh,
        night_eh,
        night_lev,
    });

    let data = Arc::new(Mutex::new(AppData {
        co2: 0.0, temperature: 0.0, humidity: 0.0,
        pm1: 0, pm25: 0, pm10: 0,
        mqtt_enabled: mqtt_cfg.enabled, mqtt_connected: false,
        wifi_ssid: "AP-Mode".into(), wifi_ip: "192.168.4.1".into(),
        gmt_off: mqtt_cfg.gmt_off, dst_off: mqtt_cfg.dst_off,
        night_on, night_sh, night_eh, night_lev,
        ap_up: true,
    }));

// ─── TFT Display (ST7735s) + splash (C++ setup order) ─────────
    let (mut tft, bl_pwm, bl_max, _bl_timer) = {
        let spi_driver = esp_idf_svc::hal::spi::SpiDriver::new(
            peripherals.spi2,
            peripherals.pins.gpio1,
            peripherals.pins.gpio2,
            Option::<AnyIOPin>::None,
            &esp_idf_svc::hal::spi::config::DriverConfig::new(),
        ).unwrap();
        let dc = PinDriver::output(peripherals.pins.gpio4).unwrap();
        let rst = PinDriver::output(peripherals.pins.gpio3).unwrap();
        // Backlight on PWM (LEDC CH0 / TIMER0, 10-bit) so night mode can dim it.
        // The timer driver must outlive the channel driver: dropping it resets
        // the timer and would kill the PWM output.
        let timer = esp_idf_svc::hal::ledc::LedcTimerDriver::new(
            peripherals.ledc.timer0,
            &esp_idf_svc::hal::ledc::config::TimerConfig::new()
                .frequency(5_u32.kHz().into())
                .resolution(esp_idf_svc::hal::ledc::Resolution::Bits10),
        ).unwrap();
        let mut bl_pwm = esp_idf_svc::hal::ledc::LedcDriver::new(
            peripherals.ledc.channel0,
            &timer,
            peripherals.pins.gpio6,
        ).unwrap();
        let bl_max = bl_pwm.get_max_duty();
        bl_pwm.set_duty(bl_max).unwrap();
        info!("Display rotated_180: {}", rotated);
        let tft = display::init_tft(spi_driver, peripherals.pins.gpio5, dc, rst, rotated).unwrap();
        (tft, bl_pwm, bl_max, timer)
    };
    display::draw_splash_border(&mut tft, config::VERSION);
    info!("TFT initialized");

    // ─── Sensors: SCD30 then PMS5003 (pSt order) ──────────────────
    let i2c_driver = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio12,
        peripherals.pins.gpio13,
        &I2cConfig::new().baudrate(100_u32.Hz().into()),
    ).unwrap();
    let mut scd30 = sensors::Scd30Sensor::new(i2c_driver);
    let scd30_ok = scd30.init();
    let mut sy = 62;
    sy = display::splash_check(&mut tft, sy, "SCD30", scd30_ok);

    let uart = UartDriver::new(
        peripherals.uart1,
        peripherals.pins.gpio10,
        peripherals.pins.gpio11,
        Option::<AnyIOPin>::None,
        Option::<AnyIOPin>::None,
        &UartConfig::new().baudrate(9600.into()),
    ).unwrap();
    let mut pms = sensors::PmsSensor::new(uart);
    sy = display::splash_check(&mut tft, sy, "PMS5003", true);

    // ─── WiFi (C++ connectToStoredWiFi: poll WL_CONNECTED, dots on TFT) ───
    sy = display::splash_text(&mut tft, " > WiFi: ", sy);
    let wifi = Arc::new(Mutex::new(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone())).unwrap()
    ));

    let mut connected = false;
    let mut conn_net: Option<(String, String)> = None;
    let mut wifi_row = sy;
    info!(
        "WiFi list: {:?}",
        stored.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>()
    );
    for (idx, (ssid, pass)) in stored.iter().enumerate() {
        info!("Trying WiFi: {}", ssid);
        let cfg = Configuration::Client(ClientConfiguration {
            ssid: heapless::String::<32>::try_from(ssid.as_str()).unwrap(),
            password: heapless::String::<64>::try_from(pass.as_str()).unwrap(),
            ..Default::default()
        });
        {
            let mut w = wifi.lock().unwrap();
            let _ = w.stop();
            thread::sleep(Duration::from_millis(200));
            if w.set_configuration(&cfg).is_err() || w.start().is_err() {
                error!("WiFi config/start failed for {}", ssid);
                continue;
            }
            if let Err(e) = w.connect() {
                warn!("WiFi connect() queued with error for {}: {:?}", ssid, e);
            }
        }

        let mut dots: usize = 0;
        let start = Instant::now();
        let mut ok_conn = false;
        loop {
            if wifi.lock().unwrap().is_connected().unwrap_or(false) {
                ok_conn = true;
                break;
            }
            if start.elapsed() >= Duration::from_millis(config::WIFI_TIMEOUT_MS) {
                break;
            }
            thread::sleep(Duration::from_millis(500));
            dots += 1;
            display::wifi_attempt(&mut tft, wifi_row, idx as u32 + 1, ssid, dots);
        }

        if ok_conn {
            connected = true;
            conn_net = Some((ssid.clone(), pass.clone()));
            // DHCP address may not be assigned the instant is_connected() turns true.
            let mut ip = "0.0.0.0".to_string();
            for _ in 0..15 {
                let got = wifi
                    .lock()
                    .unwrap()
                    .sta_netif()
                    .get_ip_info()
                    .ok()
                    .map(|i| i.ip.to_string());
                if let Some(v) = got {
                    if v != "0.0.0.0" {
                        ip = v;
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(200));
            }
            info!("WiFi connected, ip={}", ip);
            let mut d = data.lock().unwrap();
            d.wifi_ssid = ssid.clone();
            d.wifi_ip = ip;
            let nvs2 = nvs.clone();
            let store2 = network::NvsStore::new(nvs2);
            let _ = store2.save_wifi(ssid, pass);
            break;
        }
        warn!("WiFi '{}' not connected (timeout)", ssid);
        wifi.lock().unwrap().disconnect().ok();
        wifi_row += 7;
    }

if connected {
        // Keep the config AP alive alongside STA for the first minute after boot
        // so settings stay reachable even on a firewalled/isolation LAN.
        let client_cfg = match &conn_net {
            Some((ssid, pass)) => ClientConfiguration {
                ssid: heapless::String::<32>::try_from(ssid.as_str()).unwrap(),
                password: heapless::String::<64>::try_from(pass.as_str()).unwrap(),
                ..Default::default()
            },
            None => ClientConfiguration::default(),
        };
        let ap_cfg = AccessPointConfiguration {
            ssid: heapless::String::<32>::try_from(config::AP_SSID_DEF).unwrap(),
            password: heapless::String::<64>::try_from(config::AP_PASS_DEF).unwrap(),
            auth_method: AuthMethod::WPA2Personal,
            ..Default::default()
        };
        {
            let mut w = wifi.lock().unwrap();
            let _ = w.stop();
            thread::sleep(Duration::from_millis(300));
            if w.set_configuration(&Configuration::Mixed(client_cfg.clone(), ap_cfg)).is_err() || w.start().is_err() {
                error!("AP start failed");
            } else {
                thread::sleep(Duration::from_millis(500));
                let ap_ip = w
                    .ap_netif()
                    .get_ip_info()
                    .map(|i| i.ip.to_string())
                    .unwrap_or_else(|_| "unknown".into());
                info!("AP Mode: {} ip={} (1-min window)", config::AP_SSID_DEF, ap_ip);
                // esp_wifi_start does not re-join the STA profile in AP_STA mode,
                // so drive the connection explicitly to keep internet access.
                if let Err(e) = w.connect() {
                    warn!("Mixed STA connect() error: {:?}", e);
                }
                info!("Mixed mode: STA reconnect requested");
            }
        }
        display::wifi_message(&mut tft, wifi_row, "CONNECTED", display::COLOR_GREEN);
    } else {
        info!("AP Mode: {}", config::AP_SSID_DEF);
        // Mixed AP+STA (C++ WIFI_AP_STA) so /scan works while in AP mode
        let ap_cfg = AccessPointConfiguration {
            ssid: heapless::String::<32>::try_from(config::AP_SSID_DEF).unwrap(),
            password: heapless::String::<64>::try_from(config::AP_PASS_DEF).unwrap(),
            auth_method: AuthMethod::WPA2Personal,
            ..Default::default()
        };
        let ap_cfg2 = if let Some(last) = stored.first() {
            // Keep a standby STA profile so the AP remains Mixed (scan still works)
            ClientConfiguration {
                ssid: heapless::String::<32>::try_from(last.0.as_str()).unwrap(),
                password: heapless::String::<64>::try_from(last.1.as_str()).unwrap(),
                ..Default::default()
            }
        } else {
            ClientConfiguration::default()
        };
        let cfg = Configuration::Mixed(ap_cfg2, ap_cfg);
        let mut w = wifi.lock().unwrap();
        let _ = w.stop();
        thread::sleep(Duration::from_millis(300));
        if w.set_configuration(&cfg).is_err() || w.start().is_err() {
            error!("AP start failed");
        } else {
            thread::sleep(Duration::from_millis(500));
            let ap_ip = w
                .ap_netif()
                .get_ip_info()
                .map(|i| i.ip.to_string())
                .unwrap_or_else(|_| "unknown".into());
            info!("AP Mode active, ip={}", ap_ip);
            {
                let mut d = data.lock().unwrap();
                d.wifi_ssid = config::AP_SSID_DEF.to_string();
                d.wifi_ip = ap_ip.clone();
            }
            // Background attempt: keep retrying the standby STA profile while
            // the AP is up, so the device joins if the network comes back.
            if let Err(e) = w.connect() {
                warn!("AP-fallback STA connect() error: {:?}", e);
            }
        }
        display::wifi_message(&mut tft, wifi_row, "AP MODE ACTIVE", display::COLOR_ORANGE);
    }

    // ─── AP watchdog ─────────────────────────────────────────────────
    // Runtime network policy (matches the old C++ behavior):
    //   * AP stays up while STA has not been stable-connected for 60 s (covers
    //     the boot window and any loss of the home network);
    //   * once STA is stable >= 60 s, drop the AP (STA-only);
    //   * if STA drops again later, the AP comes right back so the device stays
    //     reachable for configuration.
    {
        let wifi_w = wifi.clone();
        let data_w = data.clone();
        let base_profile = conn_net.clone().or_else(|| stored.first().cloned());
        thread::spawn(move || {
            let mut ap_up = true; // boot always starts in Mixed (both branches)
            let boot = Instant::now();
            let mut sta_since: Option<Instant> = None;
            let mut sta_down_since: Option<Instant> = None;
            let sta_cfg = || match &base_profile {
                Some((ssid, pass)) => ClientConfiguration {
                    ssid: heapless::String::<32>::try_from(ssid.as_str()).unwrap(),
                    password: heapless::String::<64>::try_from(pass.as_str()).unwrap(),
                    ..Default::default()
                },
                None => ClientConfiguration::default(),
            };
            loop {
                thread::sleep(Duration::from_secs(3));
                let in_window = boot.elapsed() < Duration::from_secs(60);
                let conn = wifi_w.lock().unwrap().is_connected().unwrap_or(false);
                if conn {
                    if sta_since.is_none() {
                        sta_since = Some(Instant::now());
                    }
                    sta_down_since = None;
                } else {
                    if sta_down_since.is_none() {
                        sta_down_since = Some(Instant::now());
                    }
                    sta_since = None;
                }
                let stable_since = sta_since
                    .map(|t| t.elapsed() >= Duration::from_secs(60))
                    .unwrap_or(false);
                let lost_long = sta_down_since
                    .map(|t| t.elapsed() >= Duration::from_secs(15))
                    .unwrap_or(false);

                // Policy: never touch the AP while STA is alive. Only bring it
                // up after a real loss (>= 15 s) and hold it until STA has been
                // stable-connected for 60 s again, so weak signal does not flap.
                if ap_up {
                    if !in_window && stable_since {
                        let mut w = wifi_w.lock().unwrap();
                        if w.set_configuration(&Configuration::Client(sta_cfg())).is_err() {
                            error!("AP teardown failed");
                        } else {
                            ap_up = false;
                            data_w.lock().unwrap().ap_up = false;
                            info!("AP dropped: STA stable, STA-only");
                        }
                    }
                } else if !in_window && lost_long {
                    let mut w = wifi_w.lock().unwrap();
                    if w.set_configuration(&Configuration::Mixed(sta_cfg(), ap_cfg())).is_err() {
                        error!("AP restart failed");
                    } else {
                        if let Err(e) = w.connect() {
                            warn!("AP-restart STA connect() error: {:?}", e);
                        }
                        ap_up = true;
                        data_w.lock().unwrap().ap_up = true;
                        info!("AP up: STA lost for 15s");
                    }
                }
            }
        });
    }

    // ─── SNTP time sync (C++ configTime in both branches) ─────────────
    // Callback logs actual sync; servers are pool.ntp.org by default
    // (esp_idf_svc::sntp::SntpConf::default()).
    let _sntp = EspSntp::new_with_callback(
        &esp_idf_svc::sntp::SntpConf::default(),
        |d| info!("SNTP sync: offset {}s", d.as_secs()),
    ).unwrap();

    // ─── HTTP Server ──────────────────────────────────────────────
    let server_conf = HttpConf {
        max_uri_handlers: 20,
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&server_conf).unwrap();

    // GET /
    server.fn_handler("/", esp_idf_svc::http::Method::Get, |req| {
        let mut resp = req.into_ok_response()?;
        resp.write_all(include_str!("index.html").as_bytes())?;
        Ok::<(), esp_idf_svc::io::EspIOError>(())
    }).unwrap();

    // GET /status
    {
        let data = data.clone();
        let wifi = wifi.clone();
        let cfg = cfg.clone();
        server.fn_handler("/status", esp_idf_svc::http::Method::Get, move |req| {
            let d = data.lock().unwrap();
            let wifi_ip = {
                let w = wifi.lock().unwrap();
                let sta_ip = w.sta_netif().get_ip_info().ok().map(|i| i.ip.to_string()).filter(|v| v != "0.0.0.0");
                let ap_ip = w.ap_netif().get_ip_info().ok().map(|i| i.ip.to_string()).filter(|v| v != "0.0.0.0");
                
                if let Some(ip) = sta_ip {
                    ip
                } else if d.ap_up {
                    ap_ip.unwrap_or("192.168.4.1".to_string())
                } else if !d.wifi_ip.is_empty() && d.wifi_ip != "0.0.0.0" {
                    d.wifi_ip.clone()
                } else {
                    "no link".to_string()
                }
            };
            let co2lvl = sensors::co2_level(d.co2);
            let pm1lvl = sensors::pm_level(d.pm1);
            let pm25lvl = sensors::pm_level(d.pm25);
            let pm10lvl = sensors::pm_level(d.pm10);
            let rotated = cfg.rotated;
            let (n_on, n_sh, n_eh) = (cfg.night_on, cfg.night_sh, cfg.night_eh);
            let saved_mqtt = &cfg.mqtt;
            let saved_wifi_ssid = &cfg.wifi_ssid;
            let json = format!(
                concat!(
                    "{{\"co2\":{:.0},\"co2lvl\":\"{}\",\"co2clr\":\"{}\",",
                    "\"pm1\":{},\"pm1lvl\":\"{}\",\"pm1clr\":\"{}\",",
                    "\"pm25\":{},\"pm25lvl\":\"{}\",\"pm25clr\":\"{}\",",
                    "\"pm10\":{},\"pm10lvl\":\"{}\",\"pm10clr\":\"{}\",",
                    "\"temp\":{},\"hum\":{},\"mq\":\"{}\",\"m_en\":{},",
                    "\"gmt_h\":{},\"dst_s\":{},\"ssid\":\"{}\",\"ip\":\"{}\",",
                    "\"rot\":{},\"n_on\":{},\"n_sh\":{},\"n_eh\":{},\"n_lev\":{},\"ver\":\"{}\",",
                    "\"m_int\":{}}}"
                ),
                d.co2, co2lvl, sensors::level_color(co2lvl),
                d.pm1, pm1lvl, sensors::level_color(pm1lvl),
                d.pm25, pm25lvl, sensors::level_color(pm25lvl),
                d.pm10, pm10lvl, sensors::level_color(pm10lvl),
                if d.temperature == 0.0 { "null".to_string() } else { format!("{:.1}", d.temperature) },
                if d.humidity == 0.0 { "null".to_string() } else { format!("{:.0}", d.humidity) },
                if d.mqtt_enabled { if d.mqtt_connected { "OK" } else { "FAIL" } } else { "OFF" },
                d.mqtt_enabled,
                d.gmt_off / 3600,
                d.dst_off,
                saved_wifi_ssid,
                &wifi_ip,
                if rotated { 1 } else { 0 },
                if n_on { 1 } else { 0 },
                n_sh,
                n_eh,
                cfg.night_lev,
                config::VERSION,
                saved_mqtt.interval_sec,
            );
            let mut resp = req.into_ok_response()?;
            resp.write_all(json.as_bytes())?;
            Ok::<(), esp_idf_svc::io::EspIOError>(())
        }).unwrap();
    }

    // GET /scan
    {
        let wifi = wifi.clone();
        server.fn_handler("/scan", esp_idf_svc::http::Method::Get, move |req| {
            let mut w = wifi.lock().unwrap();
            let mut j = String::from("[");
            match w.scan() {
                Ok(networks) => {
                    for (i, ap) in networks.iter().enumerate() {
                        if i > 0 { j.push(','); }
                        j.push_str(&format!(
                            "{{\"ssid\":\"{}\",\"rssi\":{}}}",
                            ap.ssid.as_str(), ap.signal_strength
                        ));
                    }
                }
                Err(e) => {
                    error!("WiFi scan failed: {:?}", e);
                }
            }
            j.push(']');
            let mut resp = req.into_ok_response()?;
            resp.write_all(j.as_bytes())?;
            Ok::<(), esp_idf_svc::io::EspIOError>(())
        }).unwrap();
    }

    // GET /clearwifi
    {
        let nvs = nvs.clone();
        server.fn_handler("/clearwifi", esp_idf_svc::http::Method::Get, move |req| -> Result<(), esp_idf_svc::io::EspIOError> {
            let store = network::NvsStore::new(nvs.clone());
            store.clear_wifi();
            let mut resp = req.into_ok_response()?;
            resp.write_all(b"Erased. Restarting...")?;
            thread::sleep(Duration::from_secs(1));
            unsafe { esp_idf_svc::sys::esp_restart(); }
        }).unwrap();
    }

    // GET /logs — recent log lines captured in RAM ring buffer
    server.fn_handler("/logs", esp_idf_svc::http::Method::Get, move |req| -> Result<(), esp_idf_svc::io::EspIOError> {
        let mut resp = req.into_ok_response()?;
        resp.write_all(logbuf::page().as_bytes())?;
        Ok(())
    }).unwrap();

    // GET /logs/tail?after=<id> — JSON lines newer than <id>, for live append
    server.fn_handler("/logs/tail", esp_idf_svc::http::Method::Get, move |req| -> Result<(), esp_idf_svc::io::EspIOError> {
        let mut after = 0;
        let uri = req.uri();
        if let Some(q) = uri.split('?').nth(1) {
            for kv in q.split('&') {
                if let Some(v) = kv.strip_prefix("after=") {
                    after = v.parse::<u64>().unwrap_or(0);
                }
            }
        }
        let json = logbuf::tail_json(after);
        let mut resp = req.into_ok_response()?;
        resp.write_all(json.as_bytes())?;
        Ok(())
    }).unwrap();

    // GET /rotate — toggle display 180° and reboot
    {
        let nvs = nvs.clone();
        server.fn_handler("/rotate", esp_idf_svc::http::Method::Get, move |req| -> Result<(), esp_idf_svc::io::EspIOError> {
            let store = network::NvsStore::new(nvs.clone());
            let next = !store.get_display_rot();
            let _ = store.set_display_rot(next);
            let mut resp = req.into_ok_response()?;
            let msg = format!(
                "Display: {}. Restarting...",
                if next { "flipped 180" } else { "normal" }
            );
            resp.write_all(msg.as_bytes())?;
            thread::sleep(Duration::from_secs(1));
            unsafe { esp_idf_svc::sys::esp_restart(); }
        }).unwrap();
    }

    // GET /ota - HTML page for firmware upload
    server.fn_handler("/ota", esp_idf_svc::http::Method::Get, move |req| -> Result<(), esp_idf_svc::io::EspIOError> {
        let html = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>OTA Update</title><style>body{font-family:sans-serif;background:#111;color:#eee;text-align:center;padding:40px;}input,button{padding:12px;margin:10px;font-size:16px;background:#222;color:#eee;border:1px solid #444;border-radius:4px;}button{background:#0a0;cursor:pointer;}</style></head><body><h1>AirMonitor OTA Update</h1><input type="file" id="f" accept=".bin"><br><button onclick="upload()">Flash Firmware</button><p id="st"></p><br><a href="/" style="color:#8af">Back</a><script>function upload(){let f=document.getElementById('f').files[0];if(!f)return alert('Select .bin file');let st=document.getElementById('st');st.innerText='Flashing...';let xhr=new XMLHttpRequest();xhr.open('POST','/ota',true);xhr.onload=()=>{st.innerText=xhr.responseText;if(xhr.status===200){setTimeout(()=>location.href='/',5000);}};xhr.send(f);}</script></body></html>"#;
        let mut resp = req.into_ok_response()?;
        resp.write_all(html.as_bytes())?;
        Ok(())
    }).unwrap();

    // POST /ota - Receive binary and perform OTA update
    server.fn_handler("/ota", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), esp_idf_svc::io::EspIOError> {
        unsafe {
            let update_partition = esp_idf_svc::sys::esp_ota_get_next_update_partition(std::ptr::null());
            if update_partition.is_null() {
                let mut resp = req.into_ok_response()?;
                resp.write_all(b"OTA partition error")?;
                return Ok(());
            }
            let mut update_handle: esp_idf_svc::sys::esp_ota_handle_t = 0;
            let err = esp_idf_svc::sys::esp_ota_begin(update_partition, esp_idf_svc::sys::OTA_SIZE_UNKNOWN as usize, &mut update_handle);
            if err != 0 {
                let mut resp = req.into_ok_response()?;
                resp.write_all(b"esp_ota_begin failed")?;
                return Ok(());
            }

            let mut buf = vec![0u8; 1024];
            let mut ok = true;
            loop {
                match req.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let err = esp_idf_svc::sys::esp_ota_write(update_handle, buf.as_ptr() as *const _, n as usize);
                        if err != 0 {
                            ok = false;
                            break;
                        }
                    }
                }
            }

            if !ok || esp_idf_svc::sys::esp_ota_end(update_handle) != 0 {
                let mut resp = req.into_ok_response()?;
                resp.write_all(b"OTA write/end failed")?;
                return Ok(());
            }

            if esp_idf_svc::sys::esp_ota_set_boot_partition(update_partition) != 0 {
                let mut resp = req.into_ok_response()?;
                resp.write_all(b"esp_ota_set_boot_partition failed")?;
                return Ok(());
            }

            let mut resp = req.into_ok_response()?;
            resp.write_all(b"Success! Rebooting...")?;
            thread::sleep(Duration::from_secs(1));
            esp_idf_svc::sys::esp_restart();
        }
    }).unwrap();

    // POST /connect
    {
        let nvs = nvs.clone();
        server.fn_handler("/connect", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), esp_idf_svc::io::EspIOError> {
            info!("POST /connect: hit");
            let len = req.content_len().unwrap_or(2048) as usize;
            let mut buf = vec![0u8; len.min(4096)];
            let mut total = 0;
            while total < buf.len() {
                match req.read(&mut buf[total..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => total += n,
                }
            }
            let body = String::from_utf8_lossy(&buf[..total]);
            let p = parse_form(&body);
            info!("POST /connect: len={} total={} ssid={:?} has_pass={}", len, total, p.get("ssid").map(|s| s.as_str()), p.contains_key("pass"));

            if let Some(ssid) = p.get("ssid").filter(|s| !s.is_empty()) {
                let ctx = network::NvsStore::new(nvs.clone());
                let pass = p.get("pass").map(|s| s.as_str()).unwrap_or("");
                let pass = if pass.is_empty() {
                    ctx.get_wifi_list().iter().find(|(s, _)| s == ssid).map(|(_, pw)| pw.clone()).unwrap_or_default()
                } else {
                    pass.to_string()
                };
                let _ = ctx.save_wifi(ssid, &pass);
                info!("POST /connect: wifi saved ssid={}", ssid);
            }

             let prev = {
                 let store = network::NvsStore::new(nvs.clone());
                 store.load_mqtt()
             };
             let prev_pass = prev.pass.clone();
             let prev_server = prev.server.clone();
             let prev_user = prev.user.clone();
             let m_en = p.contains_key("m_en");
             let mqtt = network::MqttCfg {
                 enabled: m_en,
                 server: p.get("m_srv").cloned().unwrap_or_default(),
                 port: p.get("m_port").and_then(|v| v.parse().ok()).unwrap_or(1883),
                 user: p.get("m_user").cloned().unwrap_or_default(),
                 pass: {
                     let pw = p.get("m_pass").map(|s| s.as_str()).unwrap_or("");
                     if pw.is_empty() { prev_pass } else { pw.to_string() }
                 },
                 gmt_off: p.get("gmt_h").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0) * 3600,
                 dst_off: p.get("dst_en").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0),
                 interval_sec: p.get("m_int").and_then(|v| v.parse::<i32>().ok()).unwrap_or(10).max(1),
             };
             let mqtt = network::MqttCfg {
                 enabled: mqtt.enabled,
                 server: if mqtt.server.is_empty() { prev_server } else { mqtt.server },
                 port: if mqtt.port <= 0 { prev.port } else { mqtt.port },
                 user: if mqtt.user.is_empty() { prev_user } else { mqtt.user },
                 pass: mqtt.pass,
                 gmt_off: mqtt.gmt_off,
                 dst_off: mqtt.dst_off,
                 interval_sec: mqtt.interval_sec,
             };
            let store = network::NvsStore::new(nvs.clone());
            let _ = store.save_mqtt(&mqtt);

            // Night mode: enabled checkbox + [start, end) hours (0-23) + dim level.
            let n_sh = p.get("n_sh").and_then(|v| v.parse::<i32>().ok()).unwrap_or(config::NIGHT_START_H_DEF);
            let n_eh = p.get("n_eh").and_then(|v| v.parse::<i32>().ok()).unwrap_or(config::NIGHT_END_H_DEF);
            let n_lev = p.get("n_lev").and_then(|v| v.parse::<u32>().ok()).unwrap_or(config::NIGHT_DIM_LEVEL);
            let _ = store.set_night(p.contains_key("n_on"), n_sh.clamp(0, 23), n_eh.clamp(0, 23), n_lev.clamp(1, 1024));

            let mut resp = req.into_ok_response()?;
            resp.write_all(b"Restarting...")?;
            drop(resp);
            info!("POST /connect: response sent, restart scheduled");
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(2));
                unsafe { esp_idf_svc::sys::esp_restart(); }
            });
            Ok(())
        }).unwrap();
    }

    info!("HTTP server started");

    // C++ setup tail: delay(1000) then clear for main UI
    thread::sleep(Duration::from_millis(1000));
    display::fill_screen(&mut tft);

    // ─── loop step 2: Systems — sensor thread ─────────────────────
    {
        let data2 = data.clone();
        thread::spawn(move || {
            let scd30_ok = scd30_ok;
            let mut last_log = Instant::now();
            let mut fw_done = false;
            loop {
                thread::sleep(Duration::from_millis(500));

                if !fw_done {
                    fw_done = true;
                    info!("I2C scan ACK: {:02x?}", scd30.scan_bus());
                    for reg in [0x0202u16, 0x4600, 0xD100, 0x5100, 0x5300] {
                        if let Some(b) = scd30.read_u16_raw(reg) {
                            let crc_ok = sensors::crc8(&b[0..2]) == b[2];
                            let v = (b[0] as u16) << 8 | b[1] as u16;
                            info!("SCD30 reg {:#06x} = {:#06x} raw={:02x?} crc={}", reg, v, b, crc_ok);
                        } else {
                            info!("SCD30 reg {:#06x} read FAILED", reg);
                        }
                    }
                    if let Some(b) = scd30.read_measurement_raw() {
                        let mut crcs = String::new();
                        for i in 0..6 {
                            let ok = sensors::crc8(&b[i * 3..i * 3 + 2]) == b[i * 3 + 2];
                            crcs.push(if ok { '1' } else { '0' });
                        }
                        info!("SCD30 meas raw={:02X?} crcs={}", b, crcs);
                    }
                }

                let raw_ready = scd30.raw_data_ready();
                // data-ready flag can be unreliable on some modules: try the read
                // anyway and accept only physically plausible values.
                if raw_ready == Some(1) || scd30_ok {
                    if let Ok((c, t, h)) = scd30.read_measurement() {
                        if (400.0..=10000.0).contains(&c)
                            && (-40.0..=80.0).contains(&t)
                            && (0.0..=100.0).contains(&h)
                        {
                            let mut d = data2.lock().unwrap();
                            d.co2 = c;
                            d.temperature = t;
                            d.humidity = h;
                        }
                    }
                }

                let (mut pm1, mut pm25, mut pm10) = (0u16, 0u16, 0u16);
                pms.read_data(&mut pm1, &mut pm25, &mut pm10);
                if pm25 > 0 {
                    let mut d = data2.lock().unwrap();
                    d.pm1 = pm1;
                    d.pm25 = pm25;
                    d.pm10 = pm10;
                }

                if last_log.elapsed() >= Duration::from_secs(10) {
                    last_log = Instant::now();
                    let (co2, t, h) = {
                        let d = data2.lock().unwrap();
                        (d.co2, d.temperature, d.humidity)
                    };
                    info!(
                        "SENS: scd_ok={} raw_ready={:?} co2={:.0} t={:.1} h={:.0} pm1={} pm2.5={} pm10={}",
                        scd30_ok, raw_ready, co2, t, h, pm1, pm25, pm10
                    );
                }
            }
        });
    }

    // ─── loop step 3: MQTT thread ─────────────────────────────────
    {
        let data3 = data.clone();
        thread::spawn(move || {
            if !mqtt_cfg.enabled {
                info!("MQTT disabled (config)");
                return;
            }
            if mqtt_cfg.server.is_empty() {
                info!("MQTT enabled but server host is empty - check Settings > MQTT Broker");
                return;
            }
            let url = format!("mqtt://{}:{}", mqtt_cfg.server, mqtt_cfg.port);
            info!("MQTT connecting to {} ...", url);
            let url_for_log = url.clone();
            let client_id = format!("AirScan-{:08x}", unsafe { esp_idf_svc::sys::esp_random() });
            let username = if mqtt_cfg.user.is_empty() { None } else { Some(mqtt_cfg.user.as_str()) };
            let password = if mqtt_cfg.pass.is_empty() { None } else { Some(mqtt_cfg.pass.as_str()) };
            let mut conf = MqttClientConfiguration::default();
            conf.client_id = Some(&client_id);
            conf.username = username;
            conf.password = password;
            conf.buffer_size = 1024;

            let dc = data3.clone();
            let client = EspMqttClient::new_cb(&url, &conf, move |ev| {
                match ev.payload() {
                    EventPayload::Connected(_) => {
                        info!("MQTT connected to {}", url_for_log);
                        dc.lock().unwrap().mqtt_connected = true;
                    }
                    EventPayload::Disconnected => {
                        info!("MQTT disconnected");
                        dc.lock().unwrap().mqtt_connected = false;
                    }
                    EventPayload::Error(e) => {
                        error!("MQTT connection error: {:?}", e);
                        dc.lock().unwrap().mqtt_connected = false;
                    }
                    _ => {}
                }
            });
            let mut client = match client {
                Ok(c) => c,
                Err(e) => {
                    error!("MQTT init failed: {:?}", e);
                    return;
                }
            };
            info!("MQTT task started");

            let mut last_pub = Instant::now() - Duration::from_secs(cfg.mqtt.interval_sec as u64);
            loop {
                thread::sleep(Duration::from_millis(500));
                if !data3.lock().unwrap().mqtt_connected {
                    continue;
                }
                if last_pub.elapsed() < Duration::from_secs(cfg.mqtt.interval_sec as u64) {
                    continue;
                }
                last_pub = Instant::now();
                let (co2, temp, hum, pm1, pm25, pm10) = {
                    let d = data3.lock().unwrap();
                    (d.co2, d.temperature, d.humidity, d.pm1, d.pm25, d.pm10)
                };
                let payload = format!(
                    "{{\"co2\":{:.0},\"pm1\":{},\"pm25\":{},\"pm10\":{},\"temp\":{:.1},\"hum\":{:.0}}}",
                    co2, pm1, pm25, pm10, temp, hum
                );
                match client.publish(config::MQTT_TOPIC, QoS::AtMostOnce, false, payload.as_bytes()) {
                    Ok(_) => info!("MQTT published to {}", config::MQTT_TOPIC),
                    Err(e) => info!("MQTT publish failed: {:?}", e),
                }
            }
        });
    }

    // ─── Display loop: C++ loop() order 1 (progress), 4 (graph), 5 (clock), 6 (refresh) ───
    {
        let data4 = data.clone();
        let wifi4 = wifi.clone();
        thread::spawn(move || {
            let mut bl_pwm = bl_pwm;
            let bl_max = bl_max;
            let _bl_timer = _bl_timer; // keep PWM timer alive for channel driver
            let mut last_bw: i32 = -1;
            let mut last_duty: u32 = u32::MAX;
            let mut last_hm: u32 = u32::MAX;
            let mut last_upmin: u64 = u64::MAX;
            let mut last_has_time: Option<bool> = None;
            let mut co2_hist = [0i32; config::GRAPH_SAMPLES];
            let mut hist_idx = 0usize;
            let boot = Instant::now();
            let mut last_graph = Instant::now();
            let mut last_clock = Instant::now() - Duration::from_secs(1);
            let mut last_update = Instant::now() - Duration::from_millis(config::DISPLAY_UPDATE_INTERVAL_MS);
            let (gmt_off, dst_off) = {
                let d = data4.lock().unwrap();
                (d.gmt_off, d.dst_off)
            };

            loop {
                thread::sleep(Duration::from_millis(500));
                let now = Instant::now();

                // step 1: progress bar (width = elapsed / 5000 ms)
                let ms_since = now.duration_since(last_update).as_millis() as u32;
                let bw = ((ms_since * 128) / config::DISPLAY_UPDATE_INTERVAL_MS as u32) as i32;
                display::draw_progress_bar(&mut tft, bw, &mut last_bw);

                let (co2, temp, hum, pm1, pm25, pm10, m_en, m_con) = {
                    let d = data4.lock().unwrap();
                    (d.co2, d.temperature, d.humidity, d.pm1, d.pm25, d.pm10, d.mqtt_enabled, d.mqtt_connected)
                };

                // step 4: graph logic (every 30s push CO2 into history)
                if now.duration_since(last_graph) >= Duration::from_millis(config::GRAPH_INTERVAL_MS) {
                    last_graph = now;
                    if hist_idx < config::GRAPH_SAMPLES {
                        co2_hist[hist_idx] = co2 as i32;
                        hist_idx += 1;
                    } else {
                        co2_hist.copy_within(1.., 0);
                        co2_hist[config::GRAPH_SAMPLES - 1] = co2 as i32;
                    }
                }

                // step 5: clock (every 1s; full redraw only when minute changes)
                if now.duration_since(last_clock) >= Duration::from_secs(1) {
                    last_clock = now;
                    let uptime_s = now.duration_since(boot).as_secs();
                    let (has_time, h, m, s) = local_hms(gmt_off, dst_off);
                    if last_has_time != Some(has_time) {
                        last_has_time = Some(has_time);
                        if has_time {
                            info!("Local time acquired (uptime {}s)", uptime_s);
                        } else {
                            info!("No time yet (uptime {}s)", uptime_s);
                        }
                    }
                    if has_time {
                        let hm = h * 100 + m;
                        if hm != last_hm {
                            last_hm = hm;
                            display::draw_clock(&mut tft, true, h, m, s, uptime_s);
                        } else {
                            display::clock_colon_blink(&mut tft, s % 2 == 0);
                        }
                    } else {
                        let upmin = uptime_s / 60;
                        if upmin != last_upmin {
                            last_upmin = upmin;
                            display::draw_clock(&mut tft, false, 0, 0, 0, uptime_s);
                        } else {
                            display::clock_uptime_led(&mut tft, s % 2 == 0);
                        }
                    }

                    // step 5b: night dimming of backlight (only on time change)
                    let (n_on, n_sh, n_eh, n_lev) = {
                        let d = data4.lock().unwrap();
                        (d.night_on, d.night_sh, d.night_eh, d.night_lev)
                    };
                    // Fall back to uptime-based hour when no RTC/NTP time yet, so
                    // the dim schedule still applies after a fresh boot.
                    let dim = if n_on {
                        if has_time {
                            night_dim(h, n_sh, n_eh)
                        } else {
                            night_dim((uptime_s / 3600) as u32 % 24, n_sh, n_eh)
                        }
                    } else {
                        false
                    };
                    let target = if dim { n_lev.min(bl_max).max(1) } else { bl_max };
                    if target != last_duty {
                        last_duty = target;
                        bl_pwm.set_duty(target).ok();
                        info!("Backlight duty: {}", target);
                    }
                }

                // step 6: refresh data (every 5s)
                if now.duration_since(last_update) >= Duration::from_millis(config::DISPLAY_UPDATE_INTERVAL_MS) {
                    last_update = now;
                    let (wifi_connected, wifi_ip) = {
                        let w = wifi4.lock().unwrap();
                        let conn = w.is_connected().unwrap_or(false);
                        let last_ip = {
                            let d = data4.lock().unwrap();
                            d.wifi_ip.clone()
                        };
                        let sta_ip = w.sta_netif().get_ip_info().ok().map(|i| i.ip.to_string()).filter(|v| v != "0.0.0.0");
                        let ap_ip = w.ap_netif().get_ip_info().ok().map(|i| i.ip.to_string()).filter(|v| v != "0.0.0.0");

                        let ip_str = if let Some(ref ip) = sta_ip {
                            ip.clone()
                        } else if let Some(ref ip) = ap_ip {
                            ip.clone()
                        } else if !last_ip.is_empty() && last_ip != "0.0.0.0" {
                            last_ip
                        } else {
                            "no link".into()
                        };
                        let effective_connected = conn || sta_ip.is_some();
                        (effective_connected, ip_str)
                    };
                    display::draw_data(
                        &mut tft,
                        co2, temp, hum, pm1, pm25, pm10,
                        m_en, m_con,
                        wifi_connected,
                        &wifi_ip,
                        &co2_hist,
                    );
                }
            }
        });
    }

    info!("Running");
    loop { thread::sleep(Duration::from_secs(60)); }
}
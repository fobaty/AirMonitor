use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use esp_idf_svc::sys::EspError;

pub struct NvsStore {
    partition: EspNvsPartition<NvsDefault>,
}

impl NvsStore {
    pub fn new(partition: EspNvsPartition<NvsDefault>) -> Self {
        Self { partition }
    }

    pub fn save_wifi(&self, ssid: &str, pass: &str) -> Result<(), EspError> {
        if ssid.is_empty() {
            return Ok(());
        }
        let mut nvs = EspNvs::new(self.partition.clone(), "wifi-list", true)?;
        let mut s0_buf = [0u8; 32];
        if let Ok(Some(s0)) = nvs.get_str("s0", &mut s0_buf) {
            if s0 == ssid {
                nvs.set_str("p0", pass)?;
                return Ok(());
            }
        }
        for i in (1..5).rev() {
            let kp = format!("p{}", i - 1);
            let ks = format!("s{}", i - 1);
            let mut sb = [0u8; 32];
            let mut pb = [0u8; 64];
            if let Ok(Some(s)) = nvs.get_str(&ks, &mut sb) {
                let ss = s.to_string();
                if let Ok(Some(p)) = nvs.get_str(&kp, &mut pb) {
                    let ps = p.to_string();
                    nvs.set_str(&format!("s{}", i), &ss)?;
                    nvs.set_str(&format!("p{}", i), &ps)?;
                }
            }
        }
        nvs.set_str("s0", ssid)?;
        nvs.set_str("p0", pass)?;
        Ok(())
    }

    pub fn get_wifi_list(&self) -> Vec<(String, String)> {
        let mut nets = Vec::new();
        if let Ok(nvs) = EspNvs::new(self.partition.clone(), "wifi-list", true) {
            for i in 0..5 {
                let mut sb = [0u8; 32];
                let mut pb = [0u8; 64];
                if let Ok(Some(s)) = nvs.get_str(&format!("s{}", i), &mut sb) {
                    if !s.is_empty() {
                        let p = nvs.get_str(&format!("p{}", i), &mut pb).ok().flatten().unwrap_or("");
                        nets.push((s.to_string(), p.to_string()));
                    }
                }
            }
        }
        nets
    }

    pub fn clear_wifi(&self) {
        if let Ok(mut nvs) = EspNvs::new(self.partition.clone(), "wifi-list", true) {
            for i in 0..5 {
                let _ = nvs.remove(&format!("s{}", i));
                let _ = nvs.remove(&format!("p{}", i));
            }
        }
    }

    pub fn load_mqtt(&self) -> MqttCfg {
        let mut enabled = false;
        let mut server = String::new();
        let mut port = 1883i32;
        let mut user = String::new();
        let mut pass = String::new();
        let mut gmt = 0i32;
        let mut dst = 0i32;
        let mut interval_sec = 10i32;

        if let Ok(nvs) = EspNvs::new(self.partition.clone(), "mqtt-conf", true) {
            if let Ok(Some(v)) = nvs.get_u8("m_en") {
                enabled = v == 1;
            }
            let mut sb = [0u8; 64];
            if let Ok(Some(s)) = nvs.get_str("m_srv", &mut sb) {
                server = s.to_string();
            }
            if let Ok(Some(p)) = nvs.get_i32("m_port") {
                port = p;
            }
            let mut ub = [0u8; 32];
            if let Ok(Some(u)) = nvs.get_str("m_user", &mut ub) {
                user = u.to_string();
            }
            let mut pb = [0u8; 64];
            if let Ok(Some(pw)) = nvs.get_str("m_pass", &mut pb) {
                pass = pw.to_string();
            }
            if let Ok(Some(g)) = nvs.get_i32("gmt_off") {
                gmt = g;
            }
            if let Ok(Some(d)) = nvs.get_i32("dst_off") {
                dst = d;
            }
            if let Ok(Some(i)) = nvs.get_i32("m_int") {
                interval_sec = i;
            }
        }

        MqttCfg { enabled, server, port, user, pass, gmt_off: gmt, dst_off: dst, interval_sec: interval_sec.max(1) }
    }

    pub fn save_mqtt(&self, cfg: &MqttCfg) -> Result<(), EspError> {
        let mut nvs = EspNvs::new(self.partition.clone(), "mqtt-conf", true)?;
        nvs.set_u8("m_en", if cfg.enabled { 1 } else { 0 })?;
        nvs.set_str("m_srv", &cfg.server)?;
        nvs.set_i32("m_port", cfg.port)?;
        nvs.set_str("m_user", &cfg.user)?;
        nvs.set_str("m_pass", &cfg.pass)?;
        nvs.set_i32("gmt_off", cfg.gmt_off)?;
        nvs.set_i32("dst_off", cfg.dst_off)?;
        nvs.set_i32("m_int", cfg.interval_sec.max(1))?;
        Ok(())
    }

    // Display rotation: true = panel physically flipped (180°). Default flipped.
    pub fn get_display_rot(&self) -> bool {
        if let Ok(nvs) = EspNvs::new(self.partition.clone(), "disp", true) {
            if let Ok(Some(v)) = nvs.get_u8("rot") {
                return v == 1;
            }
        }
        true
    }

    pub fn set_display_rot(&self, rot: bool) -> Result<(), EspError> {
        let nvs = EspNvs::new(self.partition.clone(), "disp", true)?;
        nvs.set_u8("rot", if rot { 1 } else { 0 })
    }

    // Night mode: dim backlight inside [start_h, end_h). start_h == end_h means
    // "always dim". Default disabled. lev is the dimmed PWM duty (1..1024).
    pub fn get_night(&self) -> (bool, i32, i32, u32) {
        if let Ok(nvs) = EspNvs::new(self.partition.clone(), "disp", true) {
            let on = nvs.get_u8("n_on").ok().flatten().map(|v| v == 1).unwrap_or(crate::config::NIGHT_ON_DEF);
            let sh = nvs.get_i32("n_sh").ok().flatten().unwrap_or(crate::config::NIGHT_START_H_DEF);
            let eh = nvs.get_i32("n_eh").ok().flatten().unwrap_or(crate::config::NIGHT_END_H_DEF);
            let lev = nvs.get_u32("n_lev").ok().flatten().unwrap_or(crate::config::NIGHT_DIM_LEVEL).clamp(1, 1024);
            return (on, sh, eh, lev);
        }
        (crate::config::NIGHT_ON_DEF, crate::config::NIGHT_START_H_DEF, crate::config::NIGHT_END_H_DEF, crate::config::NIGHT_DIM_LEVEL)
    }

    pub fn set_night(&self, on: bool, sh: i32, eh: i32, lev: u32) -> Result<(), EspError> {
        let nvs = EspNvs::new(self.partition.clone(), "disp", true)?;
        nvs.set_u8("n_on", if on { 1 } else { 0 })?;
        nvs.set_i32("n_sh", sh)?;
        nvs.set_i32("n_eh", eh)?;
        nvs.set_u32("n_lev", lev.clamp(1, 1024))
    }
}

#[derive(Clone)]
pub struct MqttCfg {
    pub enabled: bool,
    pub server: String,
    pub port: i32,
    pub user: String,
    pub pass: String,
    pub gmt_off: i32,
    pub dst_off: i32,
    pub interval_sec: i32,
}

use ::log::{Level, LevelFilter, Metadata, Record};
use std::io::Write as _;
use std::sync::Mutex;

const MAX_LINES: usize = 300;

static LINES: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

static CURRENT_GMT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static CURRENT_DST: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub fn set_time_offsets(gmt: i32, dst: i32) {
    CURRENT_GMT.store(gmt, std::sync::atomic::Ordering::Relaxed);
    CURRENT_DST.store(dst, std::sync::atomic::Ordering::Relaxed);
}

fn get_wall_time(gmt_off: i32, dst_off: i32) -> Option<(u32, u32, u32, u32, u32, u32)> {
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if now_s == 0 {
        return None;
    }
    let shifted = now_s + gmt_off as i64 + dst_off as i64;
    let days = shifted.div_euclid(86400);
    let sod = shifted.rem_euclid(86400);
    
    let hour = (sod / 3600) as u32;
    let minute = ((sod % 3600) / 60) as u32;
    let second = (sod % 60) as u32;

    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = (mp as i32 + if mp < 10 { 3 } else { -9 }) as u32;
    let y = y + (if m <= 2 { 1 } else { 0 }) as i64;

    if y < 2020 {
        return None;
    }

    Some((y as u32, m as u32, d as u32, hour, minute, second))
}

struct RingLogger;

unsafe impl Send for RingLogger {}
unsafe impl Sync for RingLogger {}

fn marker(level: Level) -> &'static str {
    match level {
        Level::Error => "E",
        Level::Warn => "W",
        Level::Info => "I",
        Level::Debug => "D",
        Level::Trace => "V",
    }
}

impl ::log::Log for RingLogger {
    fn enabled(&self, _meta: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        let ts = unsafe { esp_idf_svc::sys::esp_log_timestamp() };
        let wall = get_wall_time(
            CURRENT_GMT.load(std::sync::atomic::Ordering::Relaxed),
            CURRENT_DST.load(std::sync::atomic::Ordering::Relaxed),
        );
        let time_str = match wall {
            Some((y, m, d, hh, mm, ss)) => format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, hh, mm, ss),
            None => format!("up {}ms", ts),
        };
        let line = format!(
            "{} [{}] {}: {}",
            marker(record.level()),
            time_str,
            record.metadata().target(),
            record.args()
        );
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u64;
        {
            let mut lock = LINES.lock().unwrap();
            if lock.len() >= MAX_LINES {
                lock.remove(0);
            }
            lock.push((id, line.clone()));
        }
        println!("{}", line);
        std::io::stdout().flush().ok();
    }

    fn flush(&self) {}
}

pub fn init() {
    let _ = ::log::set_logger(&RingLogger)
        .map(|()| ::log::set_max_level(LevelFilter::Info));
}

/// JSON-escape a string, producing a `"..."` literal.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Tail of the ring buffer as `{"last": <id>, "lines": ["...", ...]}`.
/// Only lines with id strictly greater than `after` are returned, so the web
/// UI can append just the new entries on each poll instead of redrawing all.
pub fn tail_json(after: u64) -> String {
    let mut items = Vec::new();
    let mut last = NEXT_ID.load(std::sync::atomic::Ordering::Relaxed) as u64;
    {
        let lock = LINES.lock().unwrap();
        for (id, line) in lock.iter() {
            if *id > after {
                items.push(json_escape(line));
            }
            last = *id;
        }
    }
    let lines = items.join(",");
    format!("{{\"last\":{},\"lines\":[{}]}}", last, lines)
}

fn page_body() -> String {
    r#"<h1>Monitoring Logs</h1>
<p id="meta">following live…, no auto page reload</p>
<pre id="log"></pre>
<script>
(function () {
  var last = 0;
  var pre = document.getElementById('log');
  var meta = document.getElementById('meta');
  function nearBottom() {
    return window.innerHeight + window.pageYOffset >= document.body.scrollHeight - 60;
  }
  function append(lines) {
    var stick = nearBottom();
    pre.textContent += lines.join('\n') + (lines.length ? '\n' : '');
    meta.textContent = 'lines: ' + pre.textContent.split('\n').filter(Boolean).length;
    if (stick) window.scrollTo(0, document.body.scrollHeight);
  }
  function poll() {
    fetch('/logs/tail?after=' + last)
      .then(function (r) { return r.json(); })
      .then(function (d) {
        if (d.lines && d.lines.length) append(d.lines);
        last = d.last;
        meta.textContent = 'live · last ' + last;
      })
      .catch(function () {
        meta.textContent = 'live · (offline, retrying)';
      });
  }
  poll();
  setInterval(poll, 2000);
})();
</script>"#.to_string()
}

pub fn page() -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"UTF-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Air Monitor Logs</title><style>\
body{{font-family:monospace;background:#111;color:#ddd;padding:10px;font-size:12px;}}\
h1{{font-size:16px;color:#8af;}}a{{color:#8af;}}\
pre{{white-space:pre-wrap;word-break:break-all;}}\
</style></head><body>\
<a href=\"/\">← Back</a>{}\
</body></html>",
        page_body()
    )
}
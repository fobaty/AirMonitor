use ::log::{Level, LevelFilter, Metadata, Record};
use std::io::Write as _;
use std::sync::Mutex;

const MAX_LINES: usize = 300;

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

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
        let line = format!(
            "{} ({}) {}: {}",
            marker(record.level()),
            ts,
            record.metadata().target(),
            record.args()
        );
        {
            let mut lock = LINES.lock().unwrap();
            if lock.len() >= MAX_LINES {
                lock.remove(0);
            }
            lock.push(line.clone());
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

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn page() -> String {
    let mut body = String::new();
    {
        let lock = LINES.lock().unwrap();
        for line in lock.iter().rev().take(200) {
            body.push_str(&escape(line));
            body.push('\n');
        }
    }
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"UTF-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta http-equiv=\"refresh\" content=\"2\">\
<title>Air Monitor Logs</title><style>\
body{{font-family:monospace;background:#111;color:#ddd;padding:10px;font-size:12px;}}\
h1{{font-size:16px;color:#8af;}}a{{color:#8af;}}\
pre{{white-space:pre-wrap;word-break:break-all;}}\
</style></head><body>\
<h1>Monitoring Logs (auto-refresh 2s)</h1><a href=\"/\">← Back</a><pre>{}</pre>\
</body></html>",
        body
    )
}
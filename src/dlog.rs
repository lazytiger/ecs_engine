use log::{LevelFilter, Log, Metadata, Record};

#[repr(C)]
pub struct LogParam {
    pub enabled: extern "C" fn(&Metadata) -> bool,
    pub log: extern "C" fn(&Record),
    pub flush: extern "C" fn(),
    pub level: LevelFilter,
}

struct DLog;

static mut PARAM: Option<LogParam> = None;

pub fn init(param: LogParam) {
    let level = param.level;
    unsafe {
        if PARAM.is_some() {
            eprint!("log should only init once");
            return;
        }
        PARAM.replace(param);
    }
    if let Err(err) = log::set_logger(&LOGGER).map(|_| log::set_max_level(level)) {
        eprint!("set logger failed:{}", err);
    }
}

fn param() -> &'static LogParam {
    unsafe { PARAM.as_ref().unwrap() }
}

impl Log for DLog {
    fn enabled(&self, metadata: &Metadata) -> bool {
        (param().enabled)(metadata)
    }

    fn log(&self, record: &Record) {
        (param().log)(record)
    }

    fn flush(&self) {
        (param().flush)()
    }
}

static LOGGER: DLog = DLog;

#[no_mangle]
extern "C" fn enabled(meta: &Metadata) -> bool {
    log::logger().enabled(meta)
}

#[no_mangle]
extern "C" fn log(record: &Record) {
    log::logger().log(record)
}

#[no_mangle]
extern "C" fn flush() {
    log::logger().flush()
}

pub fn log_param() -> LogParam {
    LogParam {
        enabled,
        log,
        flush,
        level: log::max_level(),
    }
}

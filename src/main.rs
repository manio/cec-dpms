use clap::Parser;
use signal_hook::{consts::SIGINT, consts::SIGTERM, consts::SIGUSR1, consts::SIGUSR2};
use simplelog::*;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{thread, time};
use hostname;

extern crate cec_rs;
use cec_rs::{
    CecCommand, CecConnectionCfgBuilder, CecDeviceType, CecDeviceTypeVec, CecLogMessage,
    CecLogicalAddress,
};

#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
struct Args {
    /// Enable debug info
    #[clap(short, long)]
    debug: bool,

    /// input device path/name of CEC device
    #[clap(short, long, parse(from_os_str))]
    input: Option<std::path::PathBuf>,
}

fn logging_init(debug: bool) {
    let conf = ConfigBuilder::new()
        .set_time_format("%F, %H:%M:%S%.3f".to_string())
        .set_write_log_enable_colors(true)
        .build();

    let mut loggers = vec![];

    let console_logger: Box<dyn SharedLogger> = TermLogger::new(
        if debug {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        },
        conf.clone(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    );
    loggers.push(console_logger);

    CombinedLogger::init(loggers).expect("Cannot initialize logging subsystem");
}

fn on_command_received(command: CecCommand) {
    debug!(
        "onCommandReceived: opcode: {:?}, initiator: {:?}",
        command.opcode, command.initiator
    );
}

fn on_log_message(log_message: CecLogMessage) {
    let log_prefix = "<black>libcec:</>";
    match log_message.level {
        cec_rs::CecLogLevel::All => trace!("{} {}", log_prefix, log_message.message),
        cec_rs::CecLogLevel::Debug | cec_rs::CecLogLevel::Traffic => {
            debug!("{} {}", log_prefix, log_message.message)
        }
        cec_rs::CecLogLevel::Notice => info!("{} {}", log_prefix, log_message.message),
        cec_rs::CecLogLevel::Warning => warn!("{} {}", log_prefix, log_message.message),
        cec_rs::CecLogLevel::Error => error!("{} {}", log_prefix, log_message.message),
    }
}

/// Returns the hostname of the current system, for use with CEC `OSD Name`.
///
/// This function gets the system hostname and returns it, or in the case of a
/// retrieval error, the string "`dummy`".  Although intended for use with CEC
/// `OSD Name`, it does not truncate the returned string. The string will be
/// truncated to 14 bytes by `libcec-sys` (not including C-string trailing null)
/// when setting a CEC `OSD Name` with `device_name()`.  It's not necessary to
/// append a trailing null, as this is done by lower-level `libcec` C bindings.
///
/// ## Example
///
/// ```rust
/// # use std::io;
/// # fn main() -> io::Result<()> {
/// let name = get_osd_hostname();
/// # Ok(())
/// # }
/// ```
///
/// ## Errors
///
/// If the `hostname::get()` function encounters any form of error, the default
/// string, "`dummy`", will be returned; in practice this is rare to happen.
///
/// If the returned hostname contains non-Unicode characters, this is a fatal
/// error, and the program panics.
/// This should **_not_** be possible according to Internet Standards:
/// [RFC 952][1], [RFC 921][2], [RFC 1123][3], and [RFC 3492][4]
///
/// [1]: https://www.rfc-editor.org/rfc/rfc952
/// [2]: https://www.rfc-editor.org/rfc/rfc921
/// [3]: https://www.rfc-editor.org/rfc/rfc1123
/// [4]: https://www.rfc-editor.org/info/rfc3492
fn get_osd_hostname() -> String {
    let hostname_result = hostname::get();
    match hostname_result {
        Err(e) => {
            debug!("get_osd_hostname: Error getting hostname {}", e);
            "dummy".to_string() // Just use a default value
        },
        Ok(v) => {
            debug!("get_osd_hostname: Hostname {:?}", v);
            v.into_string().expect("Hostname should not contain non-Unicode chars")
        },
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    logging_init(args.debug);
    let device_path = args.input.unwrap().into_os_string().into_string().unwrap();
    info!(
        "🔘 <b>cec-dpms</> started, about to open CEC connection to: <u>{}</>",
        &device_path
    );

    let hostname = get_osd_hostname();
    info!("Hostname: <b>{:?}</>", hostname);
    let cfg = CecConnectionCfgBuilder::default()
        .port(device_path)
        .device_name(hostname.into())
        .command_received_callback(Box::new(on_command_received))
        .log_message_callback(Box::new(on_log_message))
        .device_types(CecDeviceTypeVec::new(CecDeviceType::PlaybackDevice))
        .build()
        .unwrap();
    let connection = cfg.open().unwrap();
    let usr1 = Arc::new(AtomicBool::new(false));
    let usr2 = Arc::new(AtomicBool::new(false));
    let terminate = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGUSR1, Arc::clone(&usr1))?;
    signal_hook::flag::register(SIGUSR2, Arc::clone(&usr2))?;
    signal_hook::flag::register(SIGTERM, Arc::clone(&terminate))?;
    signal_hook::flag::register(SIGINT, Arc::clone(&terminate))?;

    info!("Active source: <b>{:?}</>", connection.get_active_source());
    info!("Waiting for signals...");
    loop {
        if usr1.load(Ordering::Relaxed) {
            info!("<b><green>USR1</>: powering <b>ON</>");
            usr1.store(false, Ordering::Relaxed);
            let _ = connection.send_power_on_devices(CecLogicalAddress::Tv);
            //the following call is working the same on my samsung, idk what is more proper:
            //connection.set_active_source(CecDeviceType::PlaybackDevice);
        }
        if usr2.load(Ordering::Relaxed) {
            info!("<b><green>USR2</>: powering <b>OFF</>");
            usr2.store(false, Ordering::Relaxed);
            if connection.get_active_source() == CecLogicalAddress::Playbackdevice1 {
                let _ = connection.send_standby_devices(CecLogicalAddress::Tv);
            } else {
                info!("<i>reguest ignored</>: we are not an active source");
            }
        }
        if terminate.load(Ordering::Relaxed) {
            info!("Terminating");
            break;
        }
        thread::sleep(time::Duration::from_secs(1));
    }
    Ok(())
}

use clap::Parser;
use signal_hook::{consts::SIGINT, consts::SIGTERM, consts::SIGUSR1, consts::SIGUSR2};
use simplelog::*;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::{thread, time};

use libcec::enums::{DeviceType, LogLevel, LogicalAddress};
use libcec::{CecCallbacks, Command, ConnectionBuilder, LogMessage};

/// Maximum length of the CEC `OSD Name` accepted by libcec (without trailing null).
const OSD_NAME_MAX: usize = 14;

#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
struct Args {
    /// Enable debug info
    #[clap(short, long)]
    debug: bool,

    /// input device path/name of CEC device (autodetected when not given)
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

/// Callbacks invoked by libcec from its own worker thread (must not block).
struct Handler;

impl CecCallbacks for Handler {
    fn command_received(&self, command: &Command) {
        debug!(
            "onCommandReceived: opcode: {:?}, initiator: {:?}",
            command.opcode, command.initiator
        );
    }

    fn log_message(&self, log_message: &LogMessage) {
        let log_prefix = "<black>libcec:</>";
        match log_message.level {
            LogLevel::All | LogLevel::Other(_) => {
                trace!("{} {}", log_prefix, log_message.message)
            }
            LogLevel::Debug | LogLevel::Traffic => {
                debug!("{} {}", log_prefix, log_message.message)
            }
            LogLevel::Notice => info!("{} {}", log_prefix, log_message.message),
            LogLevel::Warning => warn!("{} {}", log_prefix, log_message.message),
            LogLevel::Error => error!("{} {}", log_prefix, log_message.message),
        }
    }
}

/// Returns the hostname of the current system, for use with CEC `OSD Name`.
///
/// On a retrieval error the string "`dummy`" is returned. The result is
/// truncated to [`OSD_NAME_MAX`] characters, because libcec rejects longer
/// names instead of silently truncating them.
fn get_osd_hostname() -> String {
    let name = match hostname::get() {
        Err(e) => {
            debug!("get_osd_hostname: Error getting hostname {}", e);
            "dummy".to_string() // Just use a default value
        }
        Ok(v) => {
            debug!("get_osd_hostname: Hostname {:?}", v);
            v.to_string_lossy().into_owned()
        }
    };
    name.chars().take(OSD_NAME_MAX).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    logging_init(args.debug);
    let device_path: Option<String> = args.input.map(|p| p.to_string_lossy().into_owned());
    info!(
        "🔘 <b>cec-dpms</> started, about to open CEC connection to: <u>{}</>",
        device_path.as_deref().unwrap_or("<autodetect>")
    );

    let hostname = get_osd_hostname();
    info!("Hostname: <b>{:?}</>", hostname);
    let connection = ConnectionBuilder::new(hostname)
        .device_type(DeviceType::PlaybackDevice)
        .callbacks(Arc::new(Handler))
        .open(device_path.as_deref(), Duration::from_secs(10))?;

    let usr1 = Arc::new(AtomicBool::new(false));
    let usr2 = Arc::new(AtomicBool::new(false));
    let terminate = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGUSR1, Arc::clone(&usr1))?;
    signal_hook::flag::register(SIGUSR2, Arc::clone(&usr2))?;
    signal_hook::flag::register(SIGTERM, Arc::clone(&terminate))?;
    signal_hook::flag::register(SIGINT, Arc::clone(&terminate))?;

    info!("Active source: <b>{:?}</>", connection.active_source());
    info!("Waiting for signals...");
    loop {
        if usr1.swap(false, Ordering::Relaxed) {
            info!("<b><green>USR1</>: powering <b>ON</>");
            if let Err(e) = connection.set_active_source(DeviceType::PlaybackDevice) {
                warn!("set_active_source failed: {}", e);
            }
        }
        if usr2.swap(false, Ordering::Relaxed) {
            info!("<b><green>USR2</>: powering <b>OFF</>");
            if connection.is_libcec_active_source() {
                if let Err(e) = connection.standby(LogicalAddress::Tv) {
                    warn!("standby failed: {}", e);
                }
            } else {
                info!("<i>request ignored</>: we are not an active source");
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

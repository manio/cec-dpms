use clap::Parser;
use signal_hook::{consts::SIGINT, consts::SIGTERM, consts::SIGUSR1, consts::SIGUSR2};
use simplelog::*;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{thread, time};

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

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    logging_init(args.debug);
    let device_path = args.input.unwrap().into_os_string().into_string().unwrap();
    info!(
        "🔘 <b>cec-dpms</> started, about to open CEC connection to: <u>{}</>",
        &device_path
    );

    let cfg = CecConnectionCfgBuilder::default()
        .port(device_path)
        .device_name("dummy".into())
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

use clap::Parser;
use hostname;
use signal_hook::{consts::SIGINT, consts::SIGTERM, consts::SIGUSR1, consts::SIGUSR2};
use simplelog::*;
use std::cell::RefCell;
use std::error::Error;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{thread, time};

use arrayvec::ArrayVec;
extern crate cec_rs;
use cec_rs::{
    CecCommand, CecConnection, CecConnectionCfg, CecConnectionCfgBuilder, CecDatapacket,
    CecDeviceType, CecDeviceTypeVec, CecLogMessage, CecLogicalAddress, CecOpcode,
};

use std::sync::atomic::AtomicUsize;

static GLOBAL_THREAD_COUNT: AtomicUsize = AtomicUsize::new(0);

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
    // Note that Relaxed ordering doesn't synchronize anything
    // except the global thread counter itself.
    let old_thread_count = GLOBAL_THREAD_COUNT.fetch_add(1, Ordering::Relaxed);
    // Note that this number may not be true at the moment of printing
    // because some other thread may have changed static value already.
    debug!("live threads: {}", old_thread_count + 1);

    CONNECTION.with(|connection| {
        debug!(
            "onCommandReceived: opcode type: {:?}",
            std::any::type_name_of_val(&command.opcode)
        );
        debug!("onCommandReceived: try to borrow the connection: {:?}", std::any::type_name_of_val(&connection));
        if let Some(conn) = connection.borrow().as_ref() {
            debug!(
                "onCommandReceived: Connection successfully borrowed from thread-local storage: {:?}",
                std::any::type_name_of_val(&conn)
            );
            match command.opcode {
                CecOpcode::GiveDevicePowerStatus => {
                    debug!(
                        "onCommandReceived: Got a GiveDevicePowerStatus command!!: opcode: {:?}, initiator: {:?}, destination: {:?}, ack: {:?}, eom: {:?}, parameters: {:?}, opcode_set?: {:?}, transmit_timeout: {:?}",
                        command.opcode, command.initiator, command.destination, command.ack, command.eom, command.parameters, command.opcode_set, command.transmit_timeout
                    );

                    let mut a = ArrayVec::new();
                    a.push(0x00); // CEC_POWER_STATUS_ON
                    let packet = CecDatapacket(a);

                    let _ = conn.transmit(CecCommand {
                        initiator: CecLogicalAddress::Playbackdevice1,
                        destination: command.initiator,
                        opcode: CecOpcode::ReportPowerStatus,
                        parameters: packet,
                        eom: true,
                        ack: false,
                        opcode_set: false,
                        transmit_timeout: time::Duration::from_secs(5),
                    });
                }
                CecOpcode::ReportPowerStatus => {
                    debug!(
                        "onCommandReceived: Got a ReportPowerStatus command!!: opcode: {:?}, initiator: {:?}, destination: {:?}, ack: {:?}, eom: {:?}, parameters: {:?}, opcode_set?: {:?}, transmit_timeout: {:?}",
                        command.opcode, command.initiator, command.destination, command.ack, command.eom, command.parameters, command.opcode_set, command.transmit_timeout
                    );
                }
                _ => {
                    debug!(
                        "onCommandReceived: Unknown command: opcode: {:?}, initiator: {:?}, destination: {:?}",
                        command.opcode, command.initiator, command.destination
                    );
                }
            }
        }
        else {
            debug!("<b><red>Error:</> Could not borrow the connection: {:?}", std::any::type_name_of_val(&connection));

            debug!("<b><red>Debug:</> RefCell wrapper type: {}", std::any::type_name_of_val(&connection));
            // Get the contents of RefCell
            let borrowed = connection.borrow();
            debug!("<b><red>Debug:</> After borrow() is_some()??: {:#?} (type: {})",
                borrowed.is_some(),
                std::any::type_name_of_val(&borrowed)
            );

            // Look at the Option value inside
            match *borrowed {
                Some(ref cec_conn) => {
                    debug!("Connection exists (type: {}) with:", std::any::type_name_of_val(cec_conn));
                    debug!("  - Logical addresses: {:?}", cec_conn.get_logical_addresses());
                    debug!("  - Active source: {:?}", cec_conn.get_active_source());
                    let foo = cec_conn.is_active_source(CecLogicalAddress::Playbackdevice1);
                    debug!("  - Is Playbackdevice1 active source?: {:#?}", foo);
                },
                None => {
                    debug!("Connection is None!");
                }
            }
    }
    GLOBAL_THREAD_COUNT.fetch_sub(1, Ordering::Relaxed);
})
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
        }
        Ok(v) => {
            debug!("get_osd_hostname: Hostname {:?}", v);
            v.into_string()
                .expect("Hostname should not contain non-Unicode chars")
        }
    }
}

thread_local! {
    static CONNECTION: RefCell<Option<CecConnection>> = RefCell::new(None);
    static CONNECTION_CONFIG: RefCell<Option<CecConnectionCfg>> = RefCell::new(None);
}

/// Initializes a `CecConnection` from `CONNECTION_CONFIG` and stores it in
/// thread-local storage as `CONNECTION`
///
/// This function gets the `CecConnectionCfg` from thread-local storage
/// variable: `CONNECTION_CONFIG`.
///
/// ## Example
///
/// ```rust
/// use std::io;
/// fn main() -> io::Result<()> {
///   match initialize_connection() {
///     Some(()) => { Ok(()) }
///     None => { Err("Could not open CEC connection") }
///   }
/// }
/// ```
///
/// ## Errors
///
/// None - If an error was encountered opening the CEC connection, then `None`
///        is returned.
fn initialize_connection() -> Option<()> {
    CONNECTION.with(|conn| {
        // Get mutable access to the thread_local RefCell contents and set it
        CONNECTION_CONFIG.with(|opt_config| {
            if let Some(cfg) = opt_config.borrow_mut().take() {
                match cfg.open() {
                    Ok(c) => {
                        info!("Successfully opened CEC connection");
                        *conn.borrow_mut() = Some(c);
                        Some(())
                    }
                    Err(e) => {
                        error!("Failed to initialize CEC connection: {:?}", e);
                        *conn.borrow_mut() = None;
                        None
                    }
                }
            } else {
                error!("Failed to get mutable reference to thread-local CecConnectionCfg");
                None
            }
        })
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let old_thread_count = GLOBAL_THREAD_COUNT.fetch_add(1, Ordering::Relaxed);
    debug!("live threads at start of main(): {}", old_thread_count + 1);
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
        .port(CString::new(device_path)?)
        .device_name(hostname.into())
        .command_received_callback(Box::new(on_command_received))
        .log_message_callback(Box::new(on_log_message))
        .device_types(CecDeviceTypeVec::new(CecDeviceType::PlaybackDevice))
        .build()
        .unwrap();
    CONNECTION_CONFIG.with(|config| {
        // store it in thread-local RefCell's value
        *config.borrow_mut() = Some(cfg);
    });
    // Setup signal handling flags
    let usr1 = Arc::new(AtomicBool::new(false));
    let usr2 = Arc::new(AtomicBool::new(false));
    let terminate = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGUSR1, Arc::clone(&usr1))?;
    signal_hook::flag::register(SIGUSR2, Arc::clone(&usr2))?;
    signal_hook::flag::register(SIGTERM, Arc::clone(&terminate))?;
    signal_hook::flag::register(SIGINT, Arc::clone(&terminate))?;

    // Initialize CecConnection and store it in thread-local CONNECTION
    initialize_connection();

    // Sharing same CEC connection with callback function threads, so only borrow it when needed
    CONNECTION.with(|conn| {
        // Get mutable access to the thread_local RefCell contents and set it
        // *conn.borrow_mut() = cfg.open().ok();
        // connection = cfg.open().unwrap();
        if let Some(connection) = conn.borrow().as_ref() {
            info!(
                "Am I active source? <b>{:?}</>",
                connection.is_active_source(CecLogicalAddress::Playbackdevice1)
            );
            info!("Active source: <b>{:?}</>", connection.get_active_source());
            Ok(()) as Result<(), Box<dyn Error>>
        } else {
            let err_msg = "Failed to open CEC connection";
            error!("{}", err_msg);
            Err(err_msg.to_string().into())
            // Err(Box::new(std::io::Error::new(
            //     std::io::ErrorKind::Other,
            //     err_msg,
            // )))
        }
    })?;

    let last_thread_count = &GLOBAL_THREAD_COUNT.load(Ordering::Relaxed);
    debug!("live threads at start of main(): {}", last_thread_count);
    info!("Waiting for signals...");
    loop {
        if usr1.load(Ordering::Relaxed) {
            info!("<b><green>USR1</>: powering <b>ON</>");
            let last_thread_count = &GLOBAL_THREAD_COUNT.load(Ordering::Relaxed);
            debug!("live threads at USR1 handler start: {}", last_thread_count);
            usr1.store(false, Ordering::Relaxed);
            // This apparently set active source to Tv??
            // let _ = connection.send_power_on_devices(CecLogicalAddress::Tv);
            CONNECTION.with(|conn| {
                if let Some(connection) = conn.borrow().as_ref() {
                    let power_on_devices_result =
                        connection.send_power_on_devices(CecLogicalAddress::Tv);
                    match power_on_devices_result {
                        Ok(()) => {
                            info!("<b><green>Success!</> Sent power on command to Tv");
                        }
                        Err(e) => {
                            error!(
                                "<b><red>Error:</> Failed to send power on devices command! {:?}",
                                e
                            );
                        }
                    }
                    //the following call is working the same on my samsung, idk what is more proper:
                    let set_active_source_result: Result<(), cec_rs::CecConnectionResultError> =
                        connection.set_active_source(CecDeviceType::PlaybackDevice);
                    match set_active_source_result {
                        Ok(o) => {
                            info!("<b><green>Success!</> Set active source {:?}", o);
                        }
                        Err(e) => {
                            error!("<b><red>Error:</> Failed to set active source {:?}!", e);
                        }
                    }
                    info!(
                        "<i>connection.get_logical_addresses()</i> = {:?}",
                        connection.get_logical_addresses()
                    );
                    Ok(()) as Result<(), Box<dyn Error>>
                } else {
                    let err_msg = "Failed to open CEC connection";
                    error!("{}", err_msg);
                    Err(err_msg.to_string().into())
                }
            })?;
        }
        if usr2.load(Ordering::Relaxed) {
            info!("<b><green>USR2</>: powering <b>OFF</>");
            usr2.store(false, Ordering::Relaxed);
            CONNECTION.with(|conn| {
                // Get mutable access to the thread_local RefCell contents and set it
                // *conn.borrow_mut() = cfg.open().ok();
                if let Some(connection) = conn.borrow().as_ref() {
                    info!(
                        "<b><green>Active source:</> <b>{:?}</>",
                        connection.get_active_source()
                    );
                    if connection.get_active_source() == CecLogicalAddress::Playbackdevice1 {
                        let _ = connection.send_standby_devices(CecLogicalAddress::Tv);
                    } else {
                        info!("<i>reguest ignored</>: we are not an active source");
                    }
                    Ok(()) as Result<(), Box<dyn Error>>
                } else {
                    let err_msg = "Failed to open CEC connection";
                    error!("{}", err_msg);
                    Err(err_msg.to_string().into())
                }
            })?;
        }
        if terminate.load(Ordering::Relaxed) {
            info!("Terminating");
            break;
        }
        thread::sleep(time::Duration::from_secs(1));
    }
    Ok(()) as Result<(), Box<dyn Error>>

    // Ok(())
}

use std::{
    io::{ self, IoSlice, Read, Write, stdout }, net::{ IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket }, str::FromStr, time::{self, Duration}
};
use quicli::prelude::*;
use structopt::StructOpt;

fn main() {
    let init: Init = Init::from_args();

    env_logger::Builder::new()
        .filter_level(init.log.into())
        .init();

    match init.subcommand {
        SubCommand::Receive(receive_init) => {
            receive(init.buffer_size, receive_init).unwrap();
        }
        SubCommand::Send(send_init) => {
            send(init.buffer_size, send_init).unwrap();
        }
        // SubCommand::Socket(_full_socket_init) => {
        //     unimplemented!();
        // }
    }
}

#[derive(Deserialize, Debug, structopt::StructOpt)]
#[structopt(name="socket_pipe", about="A simple socket pipe utility")]
pub struct Init {
    #[structopt(flatten)]
    subcommand: SubCommand,

    #[structopt(short, long="log", default_value="info", help="Set the log level (error, warn, info, debug, trace)")]
    log: LogLevel,

    #[structopt(long="buffer-size", default_value="32768", help="Set the buffer size for reading/writing data")]
    buffer_size: usize,
}

#[derive(Deserialize, Debug, structopt::StructOpt)]
pub enum LogLevel{
    #[structopt(name = "error")]
    Error,
    #[structopt(name = "warn")]
    Warn,
    #[structopt(name = "info")]
    Info,
    #[structopt(name = "debug")]
    Debug,
    #[structopt(name = "trace")]
    Trace,
}
impl Into<log::LevelFilter> for LogLevel {
    fn into(self) -> log::LevelFilter {
        match self {
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Trace => log::LevelFilter::Trace,
        }
    }
}
impl FromStr for LogLevel{
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim(){
            "error" | "e" => Ok(LogLevel::Error),
            "warn"  | "w" => Ok(LogLevel::Warn),
            "info"  | "i" => Ok(LogLevel::Info),
            "debug" | "d"  => Ok(LogLevel::Debug),
            "trace" | "t"  => Ok(LogLevel::Trace),
            _ => Err(anyhow::anyhow!("Invalid log level: {}", s)),
        }
    }
}

#[derive(Deserialize, Debug, structopt::StructOpt)]
pub enum SubCommand {
    #[structopt(name = "receive")]
    Receive(ReceiveInit),
    #[structopt(name = "send")]
    Send(SendInit),
    // #[structopt(name = "socket")]
    // Socket(FullSocketInit),
}


#[derive(Deserialize, Debug, structopt::StructOpt)]
pub struct SendInit {
    #[structopt(help = "The address to connect to")]
    address: String,
}

#[derive(Deserialize, Debug, structopt::StructOpt)]
pub struct FullSocketInit {}

#[derive(Deserialize, Debug, structopt::StructOpt)]
pub struct ReceiveInit {
    #[structopt(long, help = "The address to bind to", default_value = "0.0.0.0:8080")]
    bind_addr: SocketAddrV4,
    #[structopt(long, help = "Include connection information in the output")]
    include_connection_info: bool,
}

impl Default for ReceiveInit {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 8080),
            include_connection_info: false,
        }
    }
}

fn receive(buffer_size: usize, init: ReceiveInit) -> anyhow::Result<()> {
    log::info!("Binding to {}", init.bind_addr);

    let listener = TcpListener::bind(init.bind_addr).unwrap();
    let (mut socket_stream, addr) = listener.accept()?;
    log::info!("Received connection from {}", addr);


    log::debug!("locking stdout...");
    let mut stdout = stdout();
    log::debug!("ready");

    if init.include_connection_info {
        stdout.write_all(format!("connection: {}\n\n", &addr).as_bytes())?;
        stdout.flush()?;
    }
    let mut buf = vec![0_u8; buffer_size].into_boxed_slice();
    let mut last_update = time::Instant::now();
    let mut bytes_since_last_update = 0;
    loop {
        let read = socket_stream.read(&mut buf)?;
        log::debug!("Read {} bytes from socket", read);
        if read == 0 {
            log::info!("Stopping...");
            break;
        }
        log::debug!("Writing to stdout");
        stdout.write_all(&buf[..read])?;
        log::debug!("Done writing to stdout");

        bytes_since_last_update += read;

        if bytes_since_last_update >= 1024 * 1024 {
            let speed = ( (bytes_since_last_update as f64) / last_update.elapsed().as_secs_f64() ) / (1024.0);
            log::info!("Speed is around: {speed:2} kiB/s", );
            last_update = time::Instant::now();
            bytes_since_last_update = 0;
        }
    }

    stdout.flush()?;

    Ok(())
}

pub fn send(buffer_size: usize, init: SendInit) -> anyhow::Result<()> {
    let mut sender = TcpStream::connect(init.address)?;
    let mut buf = vec![0_u8; buffer_size].into_boxed_slice();
    let mut stdin = io::stdin();
    let mut last_update = time::Instant::now();
    let mut bytes_since_last_update = 0;
    let mut speed = 0.0;
    let speed_factor = 0.4;
    loop{
        let read = stdin.read(&mut buf)?;
        log::debug!("Read {} bytes from stdin", read);
        if read == 0 {
            log::info!("Stopping...");
            break;
        }
        log::debug!("Sending to socket...");
        sender.write_all(&buf[..read])?;
        log::debug!("Done sending to socket");
        bytes_since_last_update += read;

        if bytes_since_last_update >= 1024 * 1024 {
            let round_speed = ( (bytes_since_last_update as f64) / last_update.elapsed().as_secs_f64() ) / (1024.0);
            speed = speed * (1.0 - speed_factor) + round_speed * speed_factor;
            log::info!("Speed is around: {speed:2} kiB/s", );
            last_update = time::Instant::now();
            bytes_since_last_update = 0;
        }
    }
    sender.flush()?;

    Ok(())
}

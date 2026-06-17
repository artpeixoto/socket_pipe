// use std::{
//     io::{ self, IoSlice, Read, Write, prelude::*, stdout  }, net::{ IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket }, str::FromStr, sync::{Condvar, Mutex, MutexGuard}, time::{self, Duration}
// };

#[deny(clippy::unused_async)]
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
    time,
};

use anyhow::{Result, anyhow};
use quicli::prelude::*;
use structopt::StructOpt;
use tokio::{
    io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt as _, stdout},
    join,
    net::{TcpListener, TcpStream},
    pin,
    sync::{futures, mpsc},
    try_join,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let init: Init = Init::from_args();

    env_logger::Builder::new()
        .filter_level(init.log.into())
        .init();

    match init.subcommand {
        SubCommand::Receive(receive_init) => {
            receive(init.buffer_size, receive_init).await.unwrap();
        }
        SubCommand::Send(send_init) => {
            send(init.buffer_size, send_init).await.unwrap();
        } // SubCommand::Socket(_full_socket_init) => {
          //     unimplemented!();
          // }
    }
}

#[derive(Deserialize, Debug, structopt::StructOpt)]
#[structopt(name = "socket_pipe", about = "A simple socket pipe utility")]
pub struct Init {
    #[structopt(flatten)]
    subcommand: SubCommand,

    #[structopt(
        short,
        long = "log",
        default_value = "info",
        help = "Set the log level (error, warn, info, debug, trace)"
    )]
    log: LogLevel,

    #[structopt(
        long = "buffer-size",
        default_value = "4096",
        help = "Set the buffer size for reading/writing data"
    )]
    buffer_size: usize,
}

#[derive(Deserialize, Debug, structopt::StructOpt)]
pub enum LogLevel {
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
impl FromStr for LogLevel {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "error" | "e" => Ok(LogLevel::Error),
            "warn" | "w" => Ok(LogLevel::Warn),
            "info" | "i" => Ok(LogLevel::Info),
            "debug" | "d" => Ok(LogLevel::Debug),
            "trace" | "t" => Ok(LogLevel::Trace),
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

async fn receive(buffer_size: usize, init: ReceiveInit) -> Result<()> {
    log::info!("Binding to {}", init.bind_addr);

    let listener = TcpListener::bind(init.bind_addr).await?;
    let (mut socket_stream, addr) = listener.accept().await?;

    log::info!("Received connection from {}", addr);

    log::debug!("locking stdout...");
    let mut stdout = stdout();

    if init.include_connection_info {
        stdout
            .write_all(format!("connection: {}\n\n", &addr).as_bytes())
            .await?;

        stdout.flush().await?;
    }

    move_data(socket_stream, stdout, buffer_size).await?;

    Ok(())
}

pub async fn move_data(
    from: impl AsyncRead,
    to: impl AsyncWrite,
    buffer_size: usize,
) -> Result<()> {
    let (mut data_sender, mut data_receiver) = mpsc::channel(3);
    let (mut waste_sender, mut waste_receiver) = mpsc::channel(3);

    for _ in 0..3 {
        waste_sender.send(ConstCapBuf::new(buffer_size)).await?;
    }

    pin!(from);
    pin!(to);

    let reader = (async move {
        loop {
            let Some(mut data_buf) = waste_receiver.recv().await else {
                break;
            };

            let has_data =
            	data_buf
                .write(async |buf| {
                    let a = from.read(buf).await?;
                    Ok(a)
                })
                .await?;

            if !has_data {
                break;
            }

            data_sender
                .send(data_buf)
                .await
                .map_err(|_| anyhow!("fuck"))
                .unwrap();
        }
        drop(data_sender);
        Result::<(), anyhow::Error>::Ok(())
    });

    let writer = async move {
        loop {
            let Some(mut data_buf) = data_receiver.recv().await else {
                break;
            };

            to.write_all(data_buf.read()).await?;

            waste_sender.send(data_buf).await?;
        }
        to.flush().await?;

        Result::<(), anyhow::Error>::Ok(())
    };

    try_join!(reader, writer)?;

    Ok(())
}

pub async fn send(buffer_size: usize, init: SendInit) -> anyhow::Result<()> {
    let mut socket_stream = TcpStream::connect(init.address).await?;
    let mut buf = vec![0_u8; buffer_size].into_boxed_slice();
    let stdin = io::stdin();

    move_data(stdin, socket_stream, buffer_size).await?;

    Ok(())
}

// pub struct SharedBuffer<const LEN: usize>{
// 	inner: Mutex<[u8;LEN]>,
// 	filled_semaphore: std::processFutex,
// 	emptied_semaphore: Mutex<bool>,
// }

pub struct ConstCapBuf {
    storage: Box<[u8]>,
    len: usize,
}

impl ConstCapBuf {
    pub fn new(cap: usize) -> Self {
        Self {
            storage: vec![0_u8; cap].into_boxed_slice(),
            len: 0,
        }
    }
    pub fn read(&self) -> &[u8] {
        &self.storage[..self.len]
    }

    pub async fn write(
        &mut self,
        fun: impl AsyncFnOnce(&mut [u8]) -> Result<usize, io::Error>,
    ) -> Result<bool, io::Error> {
        match fun(&mut self.storage).await {
            Ok(len) => {
            	if len == 0 {
		            return Ok(false)
             	} else {
		            self.len = len;
					return Ok(true);
              	}
            },
            Err(e) => match e.kind() {
                std::io::ErrorKind::ConnectionAborted => {
                	return Ok(false);
                }
                _ => {return Err(e)}
            },
        };
    }
}

// impl ConstCapBuf{
// 	pub fn fill
// }

// impl<const LEN: usize> ConstCapBuf<LEN>{
// 	pub fn new() -> Self{
// 		Self{
// 			storage: [0_u8;_],
// 			len: 0
// 		}
// 	}
// }

// impl<const LEN: usize> SharedBuffer<LEN>{
// 	pub fn new_empty() -> Self{
// 		let mut res = Self{
// 			inner: Mutex::new([0_u8;_]),
// 			filled_semaphore: Condvar::new(),
// 			emptied_semaphore: std::sync::
// 		};
// 		res.emptied_semaphore.notify_one();
// 		res
// 	}

// 	pub fn empty_data(&mut self, func: impl FnOnce(&mut [u8])){
// 		let mutex = Mutex::new(());
// 		let mutex_lock = mutex.lock().unwrap()
// 		self.emptied_semaphore.wait_timeout_while(guard, dur, condition)
// 	}

// 	pub fn insert_data(&mut self, func: impl FnOnce(&mut [u8;LEN]) -> usize) {
// 		self.
// 	}
// }

// pub struct SharedBufferDataRef<'a>{
// 	data_lock:
// }

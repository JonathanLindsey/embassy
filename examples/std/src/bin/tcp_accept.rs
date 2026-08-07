use std::io::Write;
use std::net::TcpStream;
use std::str::from_utf8;

use clap::Parser;
use embassy_executor::{Executor, Spawner};
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config, Ipv4Address, Ipv4Cidr, StackResources};
use embassy_net_tuntap::TunTapDevice;
use embassy_time::{Duration, Timer};
use embedded_io_async::Write as _;
use heapless::Vec;
use log::*;
use rand_core::{OsRng, TryRngCore};
use static_cell::StaticCell;

#[derive(Parser)]
#[clap(version = "1.0")]
struct Opts {
    /// TAP device name
    #[clap(long, default_value = "tap0")]
    tap: String,
    /// use a static IP instead of DHCP
    #[clap(long)]
    static_ip: bool,
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, TunTapDevice>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn main_task(spawner: Spawner) {
    let opts: Opts = Opts::parse();

    // Init network device
    let device = TunTapDevice::new(&opts.tap).unwrap();

    // Choose between dhcp or static ip
    let config = if opts.static_ip {
        Config::ipv4_static(embassy_net::StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 69, 2), 24),
            dns_servers: Vec::new(),
            gateway: Some(Ipv4Address::new(192, 168, 69, 1)),
        })
    } else {
        Config::dhcpv4(Default::default())
    };

    // Generate random seed
    let mut seed = [0; 8];
    OsRng.try_fill_bytes(&mut seed).unwrap();
    let seed = u64::from_le_bytes(seed);

    // Init network stack
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(device, config, RESOURCES.init(StackResources::new()), seed);

    // Launch network task
    spawner.spawn(net_task(runner).unwrap());

    // Then we can use it!
    let mut rx_buffer = [0; 7];
    let mut tx_buffer = [0; 4096];
    let mut buf = [0; 4096];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);

        // To reproduce the issue, set_keep_alive() is required, set_timeout() is not.
        socket.set_keep_alive(Some(Duration::from_secs(1)));
        // socket.set_timeout(Some(Duration::from_secs(10)));

        info!("Listening on TCP:9999...");
        if let Err(_) = socket.accept(9999).await {
            warn!("accept error");
            continue;
        }

        info!("Accepted a connection");

        loop {
            info!("ready to read");
            match socket.read(&mut buf).await {
                Ok(0) => {
                    warn!("socket was closed");
                    break;
                },
                Ok(_) => {},
                Err(e) => {
                    warn!("read error: {:?}", e);
                    break;
                }
            }

            info!("rxd {}", from_utf8(&buf).unwrap());

            // Different bad behaviors when delay is different
            Timer::after_millis(5000).await;
            // Timer::after_millis(500).await;
        }
        info!("Closing the connection");
        socket.abort();
        info!("Flushing the RST out...");
        _ = socket.flush().await;
        info!("Finished with the socket");
    }
}

fn send_freezing_packets() -> std::io::Result<()> {
    // Server address and port is hard coded in this program
    let mut stream = TcpStream::connect("192.168.69.2:9999").unwrap();

    let packets = ["012\n", "345\n", "678\n", "901\n", "234\n", "567\n", "890\n", "hi\n"];
    for packet in packets {
        stream.write_all(packet.as_bytes())?;
    }
    Ok(())
}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("async_io", log::LevelFilter::Info)
        .format_timestamp_nanos()
        .init();

    let embassy_thread = std::thread::spawn(|| {
        let executor = EXECUTOR.init(Executor::new());
        executor.run(|spawner| {
            spawner.spawn(main_task(spawner).unwrap());
        });
    });

    send_freezing_packets().unwrap();

    embassy_thread.join().unwrap();
}

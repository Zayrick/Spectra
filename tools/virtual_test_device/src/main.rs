use std::env;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::process;
use std::time::{Duration, Instant};

use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};

const SERVICE_TYPE: &str = "_spectra-vdev._udp.local.";
const PROTOCOL_VERSION: &str = "1";
const PACKET_MAGIC: &[u8; 4] = b"SPVD";
const PACKET_VERSION: u8 = 1;
const PACKET_OPEN: u8 = 1;
const PACKET_FRAME: u8 = 2;
const PACKET_CLOSE: u8 = 3;
const PACKET_HEADER_LEN: usize = 14;
const MAX_UDP_PAYLOAD: usize = 65_507;

fn main() {
    if let Err(error) = run() {
        eprintln!("virtual_test_device: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let arguments = match Arguments::parse(env::args().skip(1))? {
        Some(arguments) => arguments,
        None => return Ok(()),
    };
    let frame_len = checked_frame_len(arguments.width, arguments.height)?;
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let endpoint = socket.local_addr()?;
    let port = endpoint.port();
    let instance_name = format!("{} ({port})", arguments.name);
    let host_name = format!("spectra-vdev-{}-{port}.local.", process::id());
    let properties = [
        ("protocol", PROTOCOL_VERSION.to_owned()),
        ("name", arguments.name.clone()),
        ("width", arguments.width.to_string()),
        ("height", arguments.height.to_string()),
    ];
    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance_name,
        &host_name,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        port,
        &properties[..],
    )?;
    let mdns = ServiceDaemon::new()?;
    mdns.disable_interface(IfKind::All)?;
    mdns.enable_interface(IfKind::LoopbackV4)?;
    mdns.register(service)?;

    println!(
        "{}: {}x{}, UDP {endpoint}",
        arguments.name, arguments.width, arguments.height,
    );
    println!("已通过 mDNS 发布 {SERVICE_TYPE}，等待 Spectra debug 内核连接");

    receive_frames(&socket, arguments.width, arguments.height, frame_len)
}

fn receive_frames(
    socket: &UdpSocket,
    width: u16,
    height: u16,
    frame_len: usize,
) -> Result<(), Box<dyn Error>> {
    let mut buffer = vec![0; PACKET_HEADER_LEN + frame_len];
    let mut stats = FrameStats::new();

    loop {
        let (length, peer) = socket.recv_from(&mut buffer)?;
        match parse_packet(&buffer[..length], width, height, frame_len) {
            Ok(Packet::Open) => {
                stats.reset();
                println!("内核已连接：{peer}");
            }
            Ok(Packet::Frame { sequence, colors }) => stats.received(peer, sequence, colors),
            Ok(Packet::Close) => println!("内核已断开：{peer}"),
            Err(error) => eprintln!("忽略来自 {peer} 的数据报：{error}"),
        }
    }
}

enum Packet<'a> {
    Open,
    Frame { sequence: u32, colors: &'a [u8] },
    Close,
}

fn parse_packet<'a>(
    packet: &'a [u8],
    width: u16,
    height: u16,
    frame_len: usize,
) -> Result<Packet<'a>, String> {
    if packet.len() < PACKET_HEADER_LEN {
        return Err("数据报短于协议头".into());
    }
    if &packet[..4] != PACKET_MAGIC {
        return Err("magic 无效".into());
    }
    if packet[4] != PACKET_VERSION {
        return Err(format!("不支持协议版本 {}", packet[4]));
    }

    let packet_width = u16::from_be_bytes([packet[6], packet[7]]);
    let packet_height = u16::from_be_bytes([packet[8], packet[9]]);
    if (packet_width, packet_height) != (width, height) {
        return Err(format!(
            "矩阵为 {packet_width}x{packet_height}，预期 {width}x{height}"
        ));
    }
    let sequence = u32::from_be_bytes([packet[10], packet[11], packet[12], packet[13]]);

    match packet[5] {
        PACKET_OPEN if packet.len() == PACKET_HEADER_LEN => Ok(Packet::Open),
        PACKET_FRAME if packet.len() == PACKET_HEADER_LEN + frame_len => Ok(Packet::Frame {
            sequence,
            colors: &packet[PACKET_HEADER_LEN..],
        }),
        PACKET_CLOSE if packet.len() == PACKET_HEADER_LEN => Ok(Packet::Close),
        PACKET_OPEN | PACKET_FRAME | PACKET_CLOSE => Err("消息长度无效".into()),
        kind => Err(format!("未知消息类型 {kind}")),
    }
}

struct FrameStats {
    interval_started: Instant,
    frames: u64,
}

impl FrameStats {
    fn new() -> Self {
        Self {
            interval_started: Instant::now(),
            frames: 0,
        }
    }

    fn received(&mut self, peer: SocketAddr, sequence: u32, colors: &[u8]) {
        self.frames += 1;
        let elapsed = self.interval_started.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }

        let fps = self.frames as f64 / elapsed.as_secs_f64();
        let first = colors.get(..3).unwrap_or(&[0, 0, 0]);
        println!(
            "{peer}: {fps:.1} FPS, seq={sequence}, first=#{:02x}{:02x}{:02x}",
            first[0], first[1], first[2]
        );
        self.interval_started = Instant::now();
        self.frames = 0;
    }

    fn reset(&mut self) {
        self.interval_started = Instant::now();
        self.frames = 0;
    }
}

struct Arguments {
    width: u16,
    height: u16,
    name: String,
}

impl Arguments {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Option<Self>, String> {
        let mut width = None;
        let mut height = None;
        let mut name = None;
        let mut arguments = arguments;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--width" => set_once(&mut width, parse_u16("width", arguments.next())?, "width")?,
                "--height" => set_once(
                    &mut height,
                    parse_u16("height", arguments.next())?,
                    "height",
                )?,
                "--name" => {
                    let value = arguments.next().ok_or("--name 缺少值")?;
                    if value.trim().is_empty() {
                        return Err("--name 不能为空".into());
                    }
                    set_once(&mut name, value, "name")?;
                }
                "-h" | "--help" => {
                    print_help();
                    return Ok(None);
                }
                _ => return Err(format!("未知参数 {argument:?}；使用 --help 查看用法")),
            }
        }

        Ok(Some(Self {
            width: width.unwrap_or(8),
            height: height.unwrap_or(4),
            name: name.unwrap_or_else(|| "Spectra Virtual Device".into()),
        }))
    }
}

fn parse_u16(name: &str, value: Option<String>) -> Result<u16, String> {
    let value = value.ok_or_else(|| format!("--{name} 缺少值"))?;
    let parsed = value
        .parse::<u16>()
        .map_err(|_| format!("--{name} 必须是 1..65535 的整数"))?;
    if parsed == 0 {
        return Err(format!("--{name} 必须大于 0"));
    }
    Ok(parsed)
}

fn set_once<T>(target: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if target.is_some() {
        return Err(format!("--{name} 不能重复"));
    }
    *target = Some(value);
    Ok(())
}

fn checked_frame_len(width: u16, height: u16) -> Result<usize, String> {
    let frame_len = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|leds| leds.checked_mul(3))
        .ok_or("矩阵尺寸过大")?;
    if PACKET_HEADER_LEN + frame_len > MAX_UDP_PAYLOAD {
        return Err("矩阵颜色帧超过单个 UDP 数据报上限".into());
    }
    Ok(frame_len)
}

fn print_help() {
    println!(
        "virtual_test_device\n\n\
         用法：\n  virtual_test_device [--width <列数>] [--height <行数>] [--name <名称>]\n\n\
         参数：\n  --width <列数>    矩阵宽度，默认 8\n  --height <行数>   矩阵高度，默认 4\n  --name <名称>     mDNS 中显示的设备名\n  -h, --help        显示帮助"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(kind: u8, width: u16, height: u16, payload: &[u8]) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.extend_from_slice(PACKET_MAGIC);
        packet.push(PACKET_VERSION);
        packet.push(kind);
        packet.extend_from_slice(&width.to_be_bytes());
        packet.extend_from_slice(&height.to_be_bytes());
        packet.extend_from_slice(&7_u32.to_be_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    #[test]
    fn parses_frame_packet() {
        let packet = packet(PACKET_FRAME, 1, 1, &[1, 2, 3]);
        match parse_packet(&packet, 1, 1, 3).unwrap() {
            Packet::Frame { sequence, colors } => {
                assert_eq!(sequence, 7);
                assert_eq!(colors, [1, 2, 3]);
            }
            _ => panic!("expected frame"),
        }
    }

    #[test]
    fn parses_matrix_arguments() {
        let arguments = Arguments::parse(
            ["--width", "12", "--height", "5"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!((arguments.width, arguments.height), (12, 5));
    }

    #[test]
    fn rejects_matrix_larger_than_udp_datagram() {
        assert!(checked_frame_len(u16::MAX, u16::MAX).is_err());
    }
}

#[cfg(not(test))]
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
#[cfg(not(test))]
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::{LazyLock, Mutex};
#[cfg(not(test))]
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
#[cfg(not(test))]
use mdns_sd::{IfKind, Receiver, ResolvedService, ServiceDaemon, ServiceEvent};

use crate::plugin::RegisteredDevice;
#[cfg(not(test))]
use crate::plugin::{PluginMetadata, PluginType};
#[cfg(not(test))]
use crate::types::DeviceCapabilities;
use crate::types::{ColorFrame, DeviceMatrix, Led, LedId};

#[cfg(not(test))]
const SERVICE_TYPE: &str = "_spectra-vdev._udp.local.";
const DRIVER_ID: &str = "@Spectra/virtual-test-device";
#[cfg(not(test))]
const PROTOCOL_VERSION: &str = "1";
const PACKET_MAGIC: &[u8; 4] = b"SPVD";
const PACKET_VERSION: u8 = 1;
const PACKET_OPEN: u8 = 1;
const PACKET_FRAME: u8 = 2;
const PACKET_CLOSE: u8 = 3;
const PACKET_HEADER_LEN: usize = 14;
const MAX_UDP_PAYLOAD: usize = 65_507;
#[cfg(not(test))]
const INITIAL_DISCOVERY_WAIT: Duration = Duration::from_millis(250);

#[cfg(not(test))]
static DISCOVERY: LazyLock<Mutex<DiscoveryState>> =
    LazyLock::new(|| Mutex::new(DiscoveryState::default()));

#[cfg(not(test))]
#[derive(Default)]
struct DiscoveryState {
    browser: Option<VirtualDeviceBrowser>,
    unavailable: bool,
}

#[cfg(not(test))]
pub(crate) fn append_discovered(devices: &mut Vec<RegisteredDevice>) {
    let mut state = DISCOVERY.lock().unwrap_or_else(|error| error.into_inner());
    if state.unavailable {
        return;
    }

    if state.browser.is_none() {
        match VirtualDeviceBrowser::new() {
            Ok(browser) => state.browser = Some(browser),
            Err(error) => {
                eprintln!("启动虚拟测试设备发现失败：{error:#}");
                state.unavailable = true;
                return;
            }
        }
    }

    let browser = state.browser.as_mut().expect("browser 已初始化");
    browser.refresh();
    devices.extend(browser.devices.values().map(AdvertisedDevice::registered));
}

#[cfg(not(test))]
struct VirtualDeviceBrowser {
    _daemon: ServiceDaemon,
    receiver: Receiver<ServiceEvent>,
    devices: HashMap<String, AdvertisedDevice>,
    first_refresh: bool,
}

#[cfg(not(test))]
impl VirtualDeviceBrowser {
    fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("启动 mDNS daemon 失败")?;
        daemon
            .disable_interface(IfKind::All)
            .context("限制 mDNS 发现接口失败")?;
        daemon
            .enable_interface(IfKind::LoopbackV4)
            .context("启用 IPv4 loopback mDNS 发现失败")?;
        let receiver = daemon
            .browse(SERVICE_TYPE)
            .context("浏览虚拟测试设备 mDNS 服务失败")?;
        Ok(Self {
            _daemon: daemon,
            receiver,
            devices: HashMap::new(),
            first_refresh: true,
        })
    }

    fn refresh(&mut self) {
        if self.first_refresh {
            self.first_refresh = false;
            let deadline = Instant::now() + INITIAL_DISCOVERY_WAIT;
            while let Ok(event) = self
                .receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                self.process(event);
                if Instant::now() >= deadline {
                    break;
                }
            }
        }

        while let Ok(event) = self.receiver.try_recv() {
            self.process(event);
        }
    }

    fn process(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::ServiceResolved(service) => {
                let fullname = service.get_fullname().to_owned();
                match AdvertisedDevice::from_service(&service) {
                    Ok(device) => {
                        self.devices.insert(fullname, device);
                    }
                    Err(error) => {
                        self.devices.remove(&fullname);
                        eprintln!("忽略无效的虚拟测试设备 {fullname:?}：{error:#}");
                    }
                }
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                self.devices.remove(&fullname);
            }
            _ => {}
        }
    }
}

#[cfg(not(test))]
struct AdvertisedDevice {
    fullname: String,
    name: String,
    endpoint: SocketAddr,
    width: u16,
    height: u16,
}

#[cfg(not(test))]
impl AdvertisedDevice {
    fn from_service(service: &ResolvedService) -> Result<Self> {
        ensure!(
            service.get_property_val_str("protocol") == Some(PROTOCOL_VERSION),
            "protocol 必须是 {PROTOCOL_VERSION}"
        );
        let width = parse_dimension(service, "width")?;
        let height = parse_dimension(service, "height")?;
        checked_frame_len(width, height)?;

        ensure!(
            service
                .get_addresses()
                .iter()
                .any(|address| address.to_ip_addr() == IpAddr::V4(Ipv4Addr::LOCALHOST)),
            "服务没有公布 127.0.0.1 地址"
        );
        let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, service.get_port()));
        let fullname = service.get_fullname().to_owned();
        let name = service
            .get_property_val_str("name")
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| service_instance_name(&fullname).to_owned());

        Ok(Self {
            fullname,
            name,
            endpoint,
            width,
            height,
        })
    }

    fn registered(&self) -> RegisteredDevice {
        RegisteredDevice {
            plugin: PluginMetadata {
                id: DRIVER_ID.into(),
                name: "Virtual Test Device".into(),
                plugin_type: PluginType::Device,
                author: "Spectra".into(),
                version: PROTOCOL_VERSION.into(),
                license: "MIT".into(),
                source: "builtin".into(),
                description: "Debug virtual RGB matrix over UDP".into(),
                hid: Vec::new(),
                path: PathBuf::new(),
            },
            id: self.fullname.as_bytes().to_vec(),
            name: self.name.clone(),
            serial_number: None,
            matrix: rectangular_matrix(self.width, self.height),
            capabilities: DeviceCapabilities {
                live: true,
                modes: Vec::new(),
            },
            data: self.endpoint.to_string().into_bytes(),
        }
    }
}

#[cfg(not(test))]
fn parse_dimension(service: &ResolvedService, key: &str) -> Result<u16> {
    let value = service
        .get_property_val_str(key)
        .with_context(|| format!("缺少 {key} 属性"))?;
    let value = value
        .parse::<u16>()
        .with_context(|| format!("{key} 不是有效的 16 位整数"))?;
    ensure!(value > 0, "{key} 必须大于 0");
    Ok(value)
}

#[cfg(not(test))]
fn service_instance_name(fullname: &str) -> &str {
    fullname
        .strip_suffix(SERVICE_TYPE)
        .unwrap_or(fullname)
        .trim_end_matches('.')
}

fn rectangular_matrix(width: u16, height: u16) -> DeviceMatrix {
    let mut cells = Vec::with_capacity(usize::from(height));
    let mut leds = Vec::with_capacity(usize::from(width) * usize::from(height));
    let mut index = 0_i64;

    for y in 0..height {
        let mut row = Vec::with_capacity(usize::from(width));
        for x in 0..width {
            let id = LedId::Integer(index);
            row.push(Some(id.clone()));
            leds.push(Led {
                id,
                name: None,
                x,
                y,
            });
            index += 1;
        }
        cells.push(row);
    }

    DeviceMatrix {
        width,
        height,
        cells,
        leds,
    }
}

fn checked_frame_len(width: u16, height: u16) -> Result<usize> {
    let frame_len = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|leds| leds.checked_mul(3))
        .context("矩阵尺寸过大")?;
    ensure!(
        PACKET_HEADER_LEN + frame_len <= MAX_UDP_PAYLOAD,
        "矩阵颜色帧超过单个 UDP 数据报上限"
    );
    Ok(frame_len)
}

pub(crate) struct LiveSession {
    socket: UdpSocket,
    name: String,
    matrix: DeviceMatrix,
    packet: Vec<u8>,
    sequence: u32,
    closed: bool,
}

impl LiveSession {
    pub(crate) fn open(device: &RegisteredDevice) -> Result<Option<Self>> {
        if device.plugin.id != DRIVER_ID {
            return Ok(None);
        }

        let endpoint = std::str::from_utf8(&device.data)
            .context("虚拟测试设备 endpoint 不是 UTF-8")?
            .parse::<SocketAddr>()
            .context("虚拟测试设备 endpoint 无效")?;
        ensure!(
            endpoint.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST),
            "虚拟测试设备 endpoint 必须是 127.0.0.1"
        );
        let frame_len = checked_frame_len(device.matrix.width, device.matrix.height)?;
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .context("绑定虚拟测试设备 UDP socket 失败")?;
        socket
            .connect(endpoint)
            .with_context(|| format!("连接虚拟测试设备 {endpoint} 失败"))?;

        let mut session = Self {
            socket,
            name: device.name.clone(),
            matrix: device.matrix.clone(),
            packet: Vec::with_capacity(PACKET_HEADER_LEN + frame_len),
            sequence: 0,
            closed: false,
        };
        session.send(PACKET_OPEN, &[])?;
        Ok(Some(session))
    }

    pub(crate) fn render(&mut self, colors: &ColorFrame) -> Result<()> {
        let expected = checked_frame_len(self.matrix.width, self.matrix.height)?;
        ensure!(
            colors.len() == expected,
            "虚拟测试设备 {} 收到的颜色帧长度无效",
            self.name
        );
        self.send(PACKET_FRAME, colors)
    }

    pub(crate) fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.send(PACKET_CLOSE, &[])
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn matrix(&self) -> &DeviceMatrix {
        &self.matrix
    }

    fn send(&mut self, kind: u8, payload: &[u8]) -> Result<()> {
        self.packet.clear();
        self.packet.extend_from_slice(PACKET_MAGIC);
        self.packet.push(PACKET_VERSION);
        self.packet.push(kind);
        self.packet
            .extend_from_slice(&self.matrix.width.to_be_bytes());
        self.packet
            .extend_from_slice(&self.matrix.height.to_be_bytes());
        self.packet.extend_from_slice(&self.sequence.to_be_bytes());
        self.packet.extend_from_slice(payload);
        self.sequence = self.sequence.wrapping_add(1);

        let sent = self
            .socket
            .send(&self.packet)
            .context("发送虚拟测试设备 UDP 数据报失败")?;
        ensure!(sent == self.packet.len(), "UDP 数据报未完整发送");
        Ok(())
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::plugin::{PluginMetadata, PluginType};
    use crate::types::DeviceCapabilities;

    #[test]
    fn builds_row_major_matrix() {
        let matrix = rectangular_matrix(2, 2);
        assert_eq!(matrix.leds.len(), 4);
        assert_eq!(matrix.cells[1][0], Some(LedId::Integer(2)));
        assert_eq!((matrix.leds[3].x, matrix.leds[3].y), (1, 1));
    }

    #[test]
    fn rejects_matrix_larger_than_udp_datagram() {
        assert!(checked_frame_len(u16::MAX, u16::MAX).is_err());
    }

    #[test]
    fn sends_session_messages_over_udp() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let endpoint = receiver.local_addr().unwrap();
        let device = RegisteredDevice {
            plugin: PluginMetadata {
                id: DRIVER_ID.into(),
                name: "Virtual Test Device".into(),
                plugin_type: PluginType::Device,
                author: "Spectra".into(),
                version: "1".into(),
                license: "MIT".into(),
                source: "builtin".into(),
                description: "Debug virtual RGB matrix over UDP".into(),
                hid: Vec::new(),
                path: PathBuf::new(),
            },
            id: b"test".to_vec(),
            name: "Virtual Test Device".into(),
            serial_number: None,
            matrix: rectangular_matrix(1, 1),
            capabilities: DeviceCapabilities {
                live: true,
                modes: Vec::new(),
            },
            data: endpoint.to_string().into_bytes(),
        };

        let mut session = LiveSession::open(&device).unwrap().unwrap();
        assert_eq!(
            session.socket.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        let mut packet = [0; PACKET_HEADER_LEN + 3];
        let (length, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, PACKET_HEADER_LEN);
        assert_eq!(packet[5], PACKET_OPEN);

        session.render(&vec![1, 2, 3]).unwrap();
        let (length, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, PACKET_HEADER_LEN + 3);
        assert_eq!(packet[5], PACKET_FRAME);
        assert_eq!(&packet[PACKET_HEADER_LEN..length], [1, 2, 3]);

        session.close().unwrap();
        let (length, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(length, PACKET_HEADER_LEN);
        assert_eq!(packet[5], PACKET_CLOSE);
    }
}

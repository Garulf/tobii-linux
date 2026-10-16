//! Standard USB descriptors as libusb presents them, and the choice of streaming pipes.

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Alternate {
    pub interface: u8,
    pub alternate: u8,
    pub interface_class: u8,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub interfaces: Vec<Alternate>,
}

/// The 18-byte device descriptor, with libusb's field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceDescriptor(pub [u8; 18]);

impl DeviceDescriptor {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        Some(Self(bytes.get(..18)?.try_into().ok()?))
    }

    fn u16_at(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.0[at], self.0[at + 1]])
    }

    pub fn vendor_id(&self) -> u16 {
        self.u16_at(8)
    }

    pub fn product_id(&self) -> u16 {
        self.u16_at(10)
    }

    pub fn num_configurations(&self) -> u8 {
        self.0[17]
    }

    /// Every field in libusb order, as `(name, value)`.
    pub fn fields(&self) -> [(&'static str, u16); 14] {
        let b = |at: usize| u16::from(self.0[at]);
        [
            ("bLength", b(0)),
            ("bDescriptorType", b(1)),
            ("bcdUSB", self.u16_at(2)),
            ("bDeviceClass", b(4)),
            ("bDeviceSubClass", b(5)),
            ("bDeviceProtocol", b(6)),
            ("bMaxPacketSize0", b(7)),
            ("idVendor", self.u16_at(8)),
            ("idProduct", self.u16_at(10)),
            ("bcdDevice", self.u16_at(12)),
            ("iManufacturer", b(14)),
            ("iProduct", b(15)),
            ("iSerialNumber", b(16)),
            ("bNumConfigurations", b(17)),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedDevice {
    pub bus: u8,
    pub address: u8,
    pub device: DeviceDescriptor,
    pub configurations: Vec<Configuration>,
}

pub fn configuration(descriptor: &nusb::descriptors::ConfigurationDescriptor) -> Configuration {
    let interfaces = descriptor
        .interface_alt_settings()
        .map(|alt| Alternate {
            interface: alt.interface_number(),
            alternate: alt.alternate_setting(),
            interface_class: alt.class(),
            endpoints: alt
                .endpoints()
                .map(|ep| Endpoint {
                    address: ep.address(),
                    attributes: ep.attributes(),
                    max_packet_size: ep.max_packet_size_raw(),
                    interval: ep.interval(),
                })
                .collect(),
        })
        .collect();
    Configuration { value: descriptor.configuration_value(), interfaces }
}

/// Device and configuration descriptors from the concatenated bytes of sysfs `descriptors`.
pub fn parse_descriptors(bytes: &[u8]) -> Option<(DeviceDescriptor, Vec<Configuration>)> {
    let device = DeviceDescriptor::parse(bytes)?;
    let mut configurations = Vec::new();
    let mut rest = &bytes[usize::from(device.0[0]).max(18)..];
    while rest.len() >= 4 {
        let total = usize::from(u16::from_le_bytes([rest[2], rest[3]]));
        let config = nusb::descriptors::ConfigurationDescriptor::new(rest.get(..total)?)?;
        configurations.push(configuration(&config));
        rest = &rest[total..];
    }
    Some((device, configurations))
}

/// Require the descriptor-confirmed streaming pipes of the researched runtime.
///
/// Other pipes, including the firmware download pipe 0x04, remain unused.
pub fn bulk_pair(alternatives: &[Alternate]) -> Result<(Endpoint, Endpoint)> {
    let matches: Vec<&Alternate> = alternatives.iter().filter(|alt| alt.interface == 0).collect();
    if matches.len() != 1 || matches[0].alternate != 0 {
        return Err(Error::invalid("Expected exactly one interface 0 / alternate 0 descriptor"));
    }
    let bulk: Vec<&Endpoint> = matches[0].endpoints.iter().filter(|ep| ep.attributes & 3 == 2).collect();
    let incoming: Vec<&&Endpoint> = bulk.iter().filter(|ep| ep.address == 0x83).collect();
    let outgoing: Vec<&&Endpoint> = bulk.iter().filter(|ep| ep.address == 0x05).collect();
    if incoming.len() != 1 || outgoing.len() != 1 {
        return Err(Error::invalid("Interface 0 requires exactly one bulk IN 0x83 and OUT 0x05"));
    }
    Ok((**incoming[0], **outgoing[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IN: Endpoint = Endpoint { address: 0x83, attributes: 2, max_packet_size: 64, interval: 0 };
    const OUT: Endpoint = Endpoint { address: 0x05, attributes: 2, max_packet_size: 64, interval: 0 };

    fn alt(interface: u8, alternate: u8, endpoints: &[Endpoint]) -> Alternate {
        Alternate { interface, alternate, interface_class: 255, endpoints: endpoints.to_vec() }
    }

    #[test]
    fn selects_interface_zero_and_streaming_pipes() {
        let video = alt(1, 0, &[Endpoint { address: 0x81, attributes: 2, max_packet_size: 64, interval: 0 }]);
        assert_eq!(bulk_pair(&[video, alt(0, 0, &[IN, OUT])]).unwrap(), (IN, OUT));
        assert!(bulk_pair(&[alt(0, 0, &[IN, OUT, IN])]).is_err());
        assert!(bulk_pair(&[alt(0, 0, &[IN, OUT]), alt(0, 1, &[IN, OUT])]).is_err());
        let extra = [
            Endpoint { address: 0x81, attributes: 3, max_packet_size: 64, interval: 0 },
            Endpoint { address: 0x04, attributes: 2, max_packet_size: 512, interval: 0 },
            IN,
            OUT,
        ];
        assert_eq!(bulk_pair(&[alt(0, 0, &extra)]).unwrap(), (IN, OUT));
        let interrupt_out = Endpoint { address: 0x05, attributes: 3, max_packet_size: 512, interval: 0 };
        for endpoints in [vec![IN], vec![OUT], vec![IN, interrupt_out]] {
            assert!(bulk_pair(&[alt(0, 0, &endpoints)]).is_err());
        }
    }

    #[test]
    fn parses_sysfs_descriptor_bytes() {
        let device = [18, 1, 0, 2, 0, 0, 0, 64, 0x04, 0x21, 0x13, 0x03, 0, 1, 1, 2, 3, 1];
        let config = [
            9, 2, 32, 0, 1, 1, 0, 0x80, 50, //
            9, 4, 0, 0, 2, 255, 0, 0, 0, //
            7, 5, 0x83, 2, 0, 2, 0, //
            7, 5, 0x05, 2, 0, 2, 0,
        ];
        let (descriptor, configurations) = parse_descriptors(&[&device[..], &config].concat()).unwrap();
        assert_eq!((descriptor.vendor_id(), descriptor.product_id()), (0x2104, 0x0313));
        assert_eq!(configurations.len(), 1);
        assert_eq!(configurations[0].value, 1);
        let pair = bulk_pair(&configurations[0].interfaces).unwrap();
        assert_eq!((pair.0.address, pair.0.max_packet_size, pair.1.address), (0x83, 512, 0x05));
    }
}

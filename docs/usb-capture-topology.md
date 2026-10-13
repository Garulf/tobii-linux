# ET5-only USB capture topology investigation

Only installed USBPcap 1.5.4.0 and Windows device APIs on albedo were used. No capture was started, no services or permissions changed, and no other driver projects were inspected.

The read-only Windows topology snapshot is retained in `evidence/private/windows-usb-topology.json`. PnP ancestry establishes the ET5 composite device is on port 2 of a TI `0451:8142` hub; that hub is under root hub `USB\ROOT_HUB30\7&30304c2c&0&0`, associated with the AMD `1022:149C` controller. `DEVPKEY_Device_Address=2` agrees with the port location; it is not established as the USB bus device address required by USBPcap.

`QueryDosDevice` finds `USBPcap1`, `USBPcap2`, and `USBPcap3`; probes 4 through 8 return Windows error 2. The USBPcap kernel driver reports Running. Neither observation associates a USBPcap control number with the identified ET5 root hub. Root numbering must not be inferred from usbipd bus IDs, hub location labels, or enumeration order.

The installed CLI's `-h` output was successfully recovered using .NET ProcessStartInfo with redirected standard streams and explicitly closed stdin. It documents `--devices` as a comma-separated list of USB addresses and `--inject-descriptors` as descriptor injection. However, no-argument topology enumeration returned neither stdout nor stderr before timeouts at 5 seconds and 45 seconds. The owned enumeration process was killed and waited for on each timeout; no capture arguments or output file were supplied. No attempt was made to terminate unrelated processes.

The exact ET5 capture-root/device-address pair remains unresolved. Starting a capture with guessed root or address could include another device, so this investigation did not do so. A successful USBPcap topology enumeration or independently verified mapping from its control device to root hub plus the hub's USB connection address is required before an ET5-only capture.

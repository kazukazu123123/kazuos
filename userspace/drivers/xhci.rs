#![no_std]
#![no_main]
include!("../runtime/driver_runtime.rs");

const MAX_DEVICES: usize = 8;
const MAX_ENDPOINTS: usize = 2;
const RING_TRBS: usize = 256;
const RING_BYTES: usize = RING_TRBS * 16;
const PORT_CHANGE_BITS: u32 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);
const TRB_TYPE_NORMAL: u32 = 1 << 10;
const TRB_TYPE_SETUP: u32 = 2 << 10;
const TRB_TYPE_DATA: u32 = 3 << 10;
const TRB_TYPE_STATUS: u32 = 4 << 10;
const TRB_TYPE_LINK: u32 = 6 << 10;
const TRB_TYPE_ENABLE_SLOT: u32 = 9 << 10;
const TRB_TYPE_DISABLE_SLOT: u32 = 10 << 10;
const TRB_TYPE_ADDRESS_DEVICE: u32 = 11 << 10;
const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12 << 10;
const TRB_TYPE_EVALUATE_CONTEXT: u32 = 13 << 10;
const EVENT_TRANSFER: u32 = 32;
const EVENT_COMMAND: u32 = 33;
const EVENT_PORT: u32 = 34;
const KEY_RELEASE: u16 = 0x100;
const MOD_SHIFT: u16 = 0x200;
const MOD_CTRL: u16 = 0x400;
const MOD_ALT: u16 = 0x800;

#[repr(C)]
#[derive(Clone, Copy)]
struct Trb {
    d0: u32,
    d1: u32,
    d2: u32,
    d3: u32,
}

#[derive(Clone, Copy)]
struct Dma {
    virt: *mut u8,
    phys: u64,
    size: usize,
}

const HID_OFFSET_NONE: u16 = u16::MAX;

#[derive(Clone, Copy)]
struct HidFormat {
    kind: u8,
    report_id: u8,
    report_bits: u16,
    button_offsets: [u16; 3],
    x_offset: u16,
    x_size: u8,
    x_absolute: bool,
    x_logical_min: i32,
    x_logical_max: i32,
    y_offset: u16,
    y_size: u8,
    y_absolute: bool,
    y_logical_min: i32,
    y_logical_max: i32,
    modifier_offset: u16,
    key_offset: u16,
    key_count: u8,
}

impl HidFormat {
    fn empty() -> Self {
        Self {
            kind: 0,
            report_id: 0,
            report_bits: 0,
            button_offsets: [HID_OFFSET_NONE; 3],
            x_offset: HID_OFFSET_NONE,
            x_size: 0,
            x_absolute: false,
            x_logical_min: 0,
            x_logical_max: 0,
            y_offset: HID_OFFSET_NONE,
            y_size: 0,
            y_absolute: false,
            y_logical_min: 0,
            y_logical_max: 0,
            modifier_offset: HID_OFFSET_NONE,
            key_offset: HID_OFFSET_NONE,
            key_count: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Endpoint {
    active: bool,
    epid: u8,
    kind: u8,
    interface: u8,
    max_packet: u16,
    interval: u8,
    format: HidFormat,
    ring: *mut Trb,
    ring_phys: u64,
    buffer: *mut u8,
    buffer_phys: u64,
    buffer_len: usize,
    enqueue: usize,
    cycle: bool,
}

#[derive(Clone, Copy)]
struct UsbDevice {
    slot: u8,
    port: u8,
    route: u32,
    route_depth: u8,
    parent_slot: u8,
    parent_port: u8,
    speed: u8,
    hub: bool,
    hub_ports: u8,
    output: *mut u8,
    output_phys: u64,
    ep0_ring: *mut Trb,
    ep0_phys: u64,
    ep0_enqueue: usize,
    ep0_cycle: bool,
    endpoints: [Option<Endpoint>; MAX_ENDPOINTS],
    keyboard_previous: [u8; 8],
    keyboard_previous_modifiers: u8,
    mouse_x: i32,
    mouse_y: i32,
    mouse_position_valid: bool,
}

#[derive(Clone, Copy)]
struct FoundEndpoint {
    kind: u8,
    interface: u8,
    endpoint: u8,
    max_packet: u16,
    interval: u8,
    report_length: u16,
    subclass: u8,
    format: HidFormat,
}

#[derive(Clone, Copy)]
struct Event {
    kind: u32,
    code: u8,
    port: u8,
    slot: u8,
    epid: u8,
    length: usize,
}

struct Controller {
    bdf: u32,
    mmio: *mut u8,
    bar_size: u64,
    op: u32,
    runtime: u32,
    doorbell: u32,
    max_ports: u8,
    context_size: usize,
    max_slots: u8,
    dcbaa: Dma,
    command: Dma,
    events: Dma,
    erst: Dma,
    input: Dma,
    command_enqueue: usize,
    command_cycle: bool,
    event_dequeue: usize,
    event_cycle: bool,
    pending_ports: u64,
    devices: [Option<UsbDevice>; MAX_DEVICES],
    mouse_fd: u64,
    keyboard_fd: u64,
}

static mut CONTROLLER: Option<Controller> = None;

pub fn kdm_info() -> KdmInfo {
    KdmInfo { name: "xhci", depends: &[] }
}

fn bdf(info: &PciDeviceInfo) -> u32 {
    ((info.bus as u32) << 16) | ((info.device as u32) << 8) | info.function as u32
}

fn find_controller() -> Option<u32> {
    let mut index = 0;
    loop {
        let mut info = PciDeviceInfo::default();
        let count = sys_pci_info(index, &mut info);
        if count == u64::MAX || index >= count {
            return None;
        }
        if info.class_code == 0x0c && info.subclass == 0x03 && info.prog_if == 0x30 {
            return Some(bdf(&info));
        }
        index += 1;
    }
}

fn alloc_dma(size: usize) -> Option<Dma> {
    let (virt, phys) = sys_dma_alloc(size)?;
    unsafe { core::ptr::write_bytes(virt, 0, size); }
    Some(Dma { virt, phys, size })
}

fn free_dma(dma: Dma) {
    sys_dma_free(dma.virt);
}

fn write_trb(ring: *mut Trb, index: usize, trb: Trb) {
    unsafe { core::ptr::write_volatile(ring.add(index), trb); }
}

fn read_trb(ring: *mut Trb, index: usize) -> Trb {
    unsafe { core::ptr::read_volatile(ring.add(index)) }
}

fn initialize_ring(dma: Dma) -> *mut Trb {
    let ring = dma.virt as *mut Trb;
    let link = Trb {
        d0: dma.phys as u32,
        d1: (dma.phys >> 32) as u32,
        d2: 0,
        d3: TRB_TYPE_LINK | 1 | (1 << 1),
    };
    write_trb(ring, RING_TRBS - 1, link);
    ring
}

fn push_ring(ring: *mut Trb, index: usize, cycle: bool, mut trb: Trb) -> (usize, bool) {
    trb.d3 = (trb.d3 & !1) | cycle as u32;
    write_trb(ring, index, trb);
    if index == RING_TRBS - 2 {
        let mut link = read_trb(ring, RING_TRBS - 1);
        link.d3 = (link.d3 & !1) | cycle as u32;
        write_trb(ring, RING_TRBS - 1, link);
        (0, !cycle)
    } else {
        (index + 1, cycle)
    }
}

fn setup_packet(bm_request: u8, request: u8, value: u16, index: u16, length: u16) -> Trb {
    Trb {
        d0: bm_request as u32 | ((request as u32) << 8) | ((value as u32) << 16),
        d1: index as u32 | ((length as u32) << 16),
        d2: 8,
        d3: TRB_TYPE_SETUP | (1 << 6) | if length == 0 { 0 } else if bm_request & 0x80 != 0 { 3 << 16 } else { 2 << 16 },
    }
}

impl Controller {
    fn read32(&self, offset: u32) -> u32 {
        unsafe { self.mmio.add(offset as usize).cast::<u32>().read_volatile() }
    }

    fn write32(&self, offset: u32, value: u32) {
        unsafe { self.mmio.add(offset as usize).cast::<u32>().write_volatile(value); }
    }

    fn op_read32(&self, offset: u32) -> u32 {
        self.read32(self.op + offset)
    }

    fn op_write32(&self, offset: u32, value: u32) {
        self.write32(self.op + offset, value)
    }

    fn runtime_write32(&self, offset: u32, value: u32) {
        self.write32(self.runtime + offset, value)
    }

    fn doorbell(&self, slot: u8, target: u8) {
        self.write32(self.doorbell + slot as u32 * 4, target as u32);
    }

    fn claim_legacy(&self, hcc_params1: u32) -> bool {
        let mut offset = ((hcc_params1 >> 16) & 0xffff) * 4;
        if offset == 0 { return true; }
        for _ in 0..64 {
            if offset < 0x20 || offset as u64 + 4 > self.bar_size { return true; }
            let value = self.read32(offset);
            if value & 0xff == 1 {
                self.write32(offset, value | (1 << 24));
                for _ in 0..1000 {
                    if self.read32(offset) & (1 << 16) == 0 { return true; }
                    sys_sleep(1);
                }
                return false;
            }
            let next = ((value >> 8) & 0xff) * 4;
            if next == 0 { break; }
            offset = next;
        }
        true
    }

    fn port_read32(&self, port: u8) -> u32 {
        self.op_read32(0x400 + (port as u32 - 1) * 0x10)
    }

    fn port_write32(&self, port: u8, value: u32) {
        self.op_write32(0x400 + (port as u32 - 1) * 0x10, value);
    }

    fn reset_port(&self, port: u8) -> Option<u8> {
        let mut status = self.port_read32(port);
        if status & 1 == 0 {
            return None;
        }
        if status & (1 << 9) == 0 {
            self.port_write32(port, status | (1 << 9));
            sys_sleep(10);
            status = self.port_read32(port);
        }
        self.port_write32(port, (status & !PORT_CHANGE_BITS) | (1 << 4));
        for _ in 0..200 {
            sys_sleep(1);
            status = self.port_read32(port);
            if status & (1 << 21) != 0 {
                self.port_write32(port, PORT_CHANGE_BITS);
                status = self.port_read32(port);
                if status & 1 != 0 && status & 2 != 0 && status & (1 << 4) == 0 {
                    return Some(((status >> 10) & 0xf) as u8);
                }
                return None;
            }
        }
        None
    }

    fn next_event(&mut self) -> Option<Event> {
        let event_ring = self.events.virt as *mut Trb;
        let trb = read_trb(event_ring, self.event_dequeue);
        if (trb.d3 & 1) != self.event_cycle as u32 {
            return None;
        }
        let kind = (trb.d3 >> 10) & 0x3f;
        let event = Event {
            kind,
            code: (trb.d2 >> 24) as u8,
            port: (trb.d0 & 0xff) as u8,
            slot: (trb.d3 >> 24) as u8,
            epid: ((trb.d3 >> 16) & 0x1f) as u8,
            length: (trb.d2 & 0x00ff_ffff) as usize,
        };
        if self.event_dequeue == RING_TRBS - 1 {
            self.event_dequeue = 0;
            self.event_cycle = !self.event_cycle;
        } else {
            self.event_dequeue += 1;
        }
        let ptr = self.events.phys + self.event_dequeue as u64 * 16;
        self.runtime_write32(0x38, ptr as u32 | 8);
        self.write32(self.runtime + 0x3c, (ptr >> 32) as u32);
        Some(event)
    }

    fn wait_command(&mut self, slot: Option<u8>) -> Option<(u8, u8)> {
        for _ in 0..500 {
            if let Some(event) = self.next_event() {
                if event.kind == EVENT_PORT {
                    if event.port != 0 && event.port <= 64 { self.pending_ports |= 1u64 << (event.port - 1); }
                } else if event.kind == EVENT_COMMAND && (slot.is_none() || slot == Some(event.slot)) {
                    return Some((event.code, event.slot));
                }
            }
            sys_sleep(1);
        }
        println!("xhci: command timeout usbcmd={:#x} usbsts={:#x} crcr={:#x} event={:#x}", self.op_read32(0), self.op_read32(4), self.op_read32(0x18), read_trb(self.events.virt as *mut Trb, 0).d3);
        None
    }

    fn command(&mut self, d0: u32, d1: u32, d2: u32, d3: u32, slot: Option<u8>) -> Option<(u8, u8)> {
        let trb = Trb { d0, d1, d2, d3 };
        let ring = self.command.virt as *mut Trb;
        (self.command_enqueue, self.command_cycle) = push_ring(ring, self.command_enqueue, self.command_cycle, trb);
        self.doorbell(0, 0);
        self.wait_command(slot)
    }

    fn control_transfer(&mut self, device: &mut UsbDevice, bm: u8, request: u8, value: u16, index: u16, buffer: *mut u8, buffer_phys: u64, length: u16) -> bool {
        let ring = device.ep0_ring;
        let mut enqueue = device.ep0_enqueue;
        let mut cycle = device.ep0_cycle;
        let mut setup = setup_packet(bm, request, value, index, length);
        setup.d3 |= 1 << 4;
        (enqueue, cycle) = push_ring(ring, enqueue, cycle, setup);
        if length != 0 {
            let data = Trb {
                d0: buffer_phys as u32,
                d1: (buffer_phys >> 32) as u32,
                d2: length as u32,
                d3: TRB_TYPE_DATA | (1 << 4) | if bm & 0x80 != 0 { 1 << 16 } else { 0 },
            };
            (enqueue, cycle) = push_ring(ring, enqueue, cycle, data);
        }
        let status = Trb {
            d0: 0,
            d1: 0,
            d2: 0,
            d3: TRB_TYPE_STATUS | (1 << 5) | if length == 0 || bm & 0x80 == 0 { 1 << 16 } else { 0 },
        };
        (enqueue, cycle) = push_ring(ring, enqueue, cycle, status);
        device.ep0_enqueue = enqueue;
        device.ep0_cycle = cycle;
        self.doorbell(device.slot, 1);
        for _ in 0..500 {
            if let Some(event) = self.next_event() {
                if event.kind == EVENT_PORT {
                    if event.port != 0 && event.port <= 64 { self.pending_ports |= 1u64 << (event.port - 1); }
                } else if event.kind == EVENT_TRANSFER && event.slot == device.slot && event.epid == 1 {
                    return event.code == 1 || event.code == 13;
                }
            }
            sys_sleep(1);
        }
        let ep = unsafe { device.output.add(self.context_size) as *const u32 };
        println!("xhci: control timeout slot {} ep0={:#x} dequeue={:#x} ring0={:#x} ring1={:#x}", device.slot, unsafe { ep.read_volatile() }, unsafe { ep.add(2).read_volatile() }, read_trb(device.ep0_ring, 0).d3, read_trb(device.ep0_ring, 1).d3);
        false
    }

    fn evaluate_hub(&mut self, device: &UsbDevice) -> bool {
        unsafe { core::ptr::write_bytes(self.input.virt, 0, self.input.size); }
        unsafe {
            let control = self.input.virt as *mut u32;
            let slot = self.input.virt.add(32) as *mut u32;
            control.add(1).write_volatile(1);
            slot.write_volatile(device.route | ((device.speed as u32) << 20) | (1 << 26) | (1 << 27));
        }
        let result = self.command(
            self.input.phys as u32,
            (self.input.phys >> 32) as u32,
            0,
            TRB_TYPE_EVALUATE_CONTEXT | ((device.slot as u32) << 24),
            Some(device.slot),
        );
        result.map_or(false, |(code, _)| code == 1)
    }

    fn evaluate_mps(&mut self, device: &UsbDevice, mps: u16) -> bool {
        unsafe { core::ptr::write_bytes(self.input.virt, 0, self.input.size); }
        let ep = unsafe { self.input.virt.add(32 + self.context_size) as *mut u32 };
        unsafe {
            (self.input.virt as *mut u32).add(1).write_volatile(1 << 1);
            ep.add(0).write_volatile(0);
            ep.add(1).write_volatile((3 << 1) | (4 << 3) | ((mps as u32) << 16));
            ep.add(2).write_volatile(device.ep0_phys as u32 | 1);
            ep.add(3).write_volatile((device.ep0_phys >> 32) as u32);
            ep.add(4).write_volatile(8);
        }
        let result = self.command(
            self.input.phys as u32,
            (self.input.phys >> 32) as u32,
            0,
            TRB_TYPE_EVALUATE_CONTEXT | ((device.slot as u32) << 24),
            Some(device.slot),
        );
        result.map_or(false, |(code, _)| code == 1)
    }

    fn configure_endpoints(&mut self, device: &UsbDevice) -> bool {
        unsafe { core::ptr::write_bytes(self.input.virt, 0, self.input.size); }
        let input = self.input.virt;
        let slot_ctx = unsafe { input.add(32) as *mut u32 };
        let add_flags = input as *mut u32;
        let mut highest = 1u8;
        unsafe {
            add_flags.add(1).write_volatile(1);
            (*slot_ctx.add(0)) = 1 << 27;
        }
        for endpoint in device.endpoints.iter().flatten() {
            highest = highest.max(endpoint.epid);
            unsafe {
                add_flags.add(1).write_volatile(add_flags.add(1).read_volatile() | (1u32 << endpoint.epid));
                let ctx = unsafe { input.add(32 + self.context_size * endpoint.epid as usize) as *mut u32 };
                ctx.add(0).write_volatile((endpoint.interval as u32) << 16);
                ctx.add(1).write_volatile((3 << 1) | (7 << 3) | ((endpoint.max_packet as u32) << 16));
                ctx.add(2).write_volatile(endpoint.ring_phys as u32 | 1);
                ctx.add(3).write_volatile((endpoint.ring_phys >> 32) as u32);
                ctx.add(4).write_volatile(endpoint.max_packet as u32);
            }
        }
        unsafe {
            slot_ctx.add(0).write_volatile(device.route | ((device.speed as u32) << 20) | ((device.hub as u32) << 26) | ((highest as u32) << 27));
            slot_ctx.add(1).write_volatile((device.port as u32) << 16);
        }
        let result = self.command(
            self.input.phys as u32,
            (self.input.phys >> 32) as u32,
            0,
            TRB_TYPE_CONFIGURE_ENDPOINT | ((device.slot as u32) << 24),
            Some(device.slot),
        );
        result.map_or(false, |(code, _)| code == 1)
    }

    fn queue_interrupt(&self, endpoint: &mut Endpoint) {
        let trb = Trb {
            d0: endpoint.buffer_phys as u32,
            d1: (endpoint.buffer_phys >> 32) as u32,
            d2: endpoint.buffer_len as u32,
            d3: TRB_TYPE_NORMAL | (1 << 5),
        };
        (endpoint.enqueue, endpoint.cycle) = push_ring(endpoint.ring, endpoint.enqueue, endpoint.cycle, trb);
    }

    fn enumerate_port(&mut self, port: u8) -> Option<UsbDevice> {
        self.enumerate_device(port, 0, 0, 0, 0, None)
    }

    fn enumerate_device(&mut self, port: u8, route: u32, route_depth: u8, parent_slot: u8, parent_port: u8, speed_override: Option<u8>) -> Option<UsbDevice> {
        if route_depth > 5 { return None; }
        if route == 0 && self.port_read32(port) & 1 == 0 { return None; }
        let speed = match speed_override.or_else(|| self.reset_port(port)) {
            Some(speed) => speed,
            None => { println!("xhci: port {} reset failed", port); return None; }
        };
        let output = alloc_dma(4096)?;
        let ep0_dma = match alloc_dma(RING_BYTES) {
            Some(value) => value,
            None => { free_dma(output); return None; }
        };
        initialize_ring(ep0_dma);
        let enabled = self.command(0, 0, 0, TRB_TYPE_ENABLE_SLOT, None);
        let (code, slot) = match enabled {
            Some(value) if value.0 == 1 && value.1 != 0 && (value.1 as usize) <= MAX_DEVICES => value,
            other => { println!("xhci: enable slot failed {:?}", other); free_dma(ep0_dma); free_dma(output); return None; }
        };
        let _ = code;
        unsafe {
            core::ptr::write_volatile((self.dcbaa.virt as *mut u64).add(slot as usize), output.phys);
            core::ptr::write_bytes(self.input.virt, 0, self.input.size);
            let control = self.input.virt as *mut u32;
            let slot_ctx = self.input.virt.add(32) as *mut u32;
            let ep_ctx = self.input.virt.add(32 + self.context_size) as *mut u32;
            control.add(1).write_volatile(3);
            slot_ctx.write_volatile(route | ((speed as u32) << 20) | (1 << 27));
            slot_ctx.add(1).write_volatile((port as u32) << 16);
            ep_ctx.write_volatile(0);
            ep_ctx.add(1).write_volatile((3 << 1) | (4 << 3) | ((initial_mps(speed) as u32) << 16));
            ep_ctx.add(2).write_volatile(ep0_dma.phys as u32 | 1);
            ep_ctx.add(3).write_volatile((ep0_dma.phys >> 32) as u32);
            ep_ctx.add(4).write_volatile(8);
        }
        let address = self.command(
            self.input.phys as u32,
            (self.input.phys >> 32) as u32,
            0,
            TRB_TYPE_ADDRESS_DEVICE | ((slot as u32) << 24),
            Some(slot),
        );
        if address.map_or(true, |value| value.0 != 1) {
            free_dma(ep0_dma);
            free_dma(output);
            return None;
        }
        let mut device = UsbDevice {
            slot,
            port,
            route,
            route_depth,
            parent_slot,
            parent_port,
            speed,
            hub: false,
            hub_ports: 0,
            output: output.virt,
            output_phys: output.phys,
            ep0_ring: ep0_dma.virt as *mut Trb,
            ep0_phys: ep0_dma.phys,
            ep0_enqueue: 0,
            ep0_cycle: true,
            endpoints: [None; MAX_ENDPOINTS],
            keyboard_previous: [0; 8],
            keyboard_previous_modifiers: 0,
            mouse_x: 0,
            mouse_y: 0,
            mouse_position_valid: false,
        };
        let descriptor = match alloc_dma(1024) {
            Some(value) => value,
            None => { free_dma(ep0_dma); free_dma(output); return None; }
        };
        if !self.control_transfer(&mut device, 0x80, 6, 0x0100, 0, descriptor.virt, descriptor.phys, 8) {
            free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
        }
        let actual_mps = unsafe { descriptor.virt.add(7).read_volatile() } as u16;
        if actual_mps == 0 || actual_mps > 1024 || (actual_mps != initial_mps(speed) && !self.evaluate_mps(&device, actual_mps)) {
            free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
        }
        if !self.control_transfer(&mut device, 0x80, 6, 0x0100, 0, descriptor.virt, descriptor.phys, 18) {
            free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
        }
        if unsafe { descriptor.virt.add(1).read_volatile() } != 1 {
            free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
        }
        if !self.control_transfer(&mut device, 0x80, 6, 0x0200, 0, descriptor.virt, descriptor.phys, 9) {
            free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
        }
        let total = unsafe {
            u16::from_le_bytes([descriptor.virt.add(2).read_volatile(), descriptor.virt.add(3).read_volatile()])
        } as usize;
        if total < 9 || total > 1024 || !self.control_transfer(&mut device, 0x80, 6, 0x0200, 0, descriptor.virt, descriptor.phys, total as u16) {
            free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
        }
        let mut found = [None; MAX_ENDPOINTS];
        parse_hid_endpoints(descriptor.virt, total, &mut found);
        let mut config_value = unsafe { descriptor.virt.add(5).read_volatile() };
        if config_value == 0 { config_value = 1; }
        for index in 0..MAX_ENDPOINTS {
            let Some(mut endpoint) = found[index] else { continue; };
            if endpoint.kind == 4 {
                if endpoint.report_length == 0 || endpoint.report_length as usize > descriptor.size || !self.control_transfer(&mut device, 0x81, 6, 0x2200, endpoint.interface as u16, descriptor.virt, descriptor.phys, endpoint.report_length) {
                    found[index] = None;
                    continue;
                }
                let Some(format) = hid_report_format(descriptor.virt, endpoint.report_length as usize) else {
                    found[index] = None;
                    continue;
                };
                let report_bytes = (format.report_bits as usize + 7) / 8 + usize::from(format.report_id != 0);
                if report_bytes == 0 || report_bytes > endpoint.max_packet as usize {
                    found[index] = None;
                    continue;
                }
                endpoint.kind = format.kind;
                endpoint.format = format;
                found[index] = Some(endpoint);
            }
        }
        let hub_ports = if found.iter().flatten().any(|endpoint| endpoint.kind == 3) {
            if !self.control_transfer(&mut device, 0xa0, 6, 0x2900, 0, descriptor.virt, descriptor.phys, 9) {
                free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
            }
            unsafe { descriptor.virt.add(2).read_volatile() }
        } else { 0 };
        let is_hub = hub_ports != 0;
        device.hub = is_hub;
        device.hub_ports = hub_ports;
        if is_hub && !self.evaluate_hub(&device) {
            free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
        }
        if !self.control_transfer(&mut device, 0, 9, config_value as u16, 0, descriptor.virt, descriptor.phys, 0) {
            free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
        }
        for endpoint in found.iter().flatten() {
            if endpoint.subclass == 1 && (endpoint.kind == 1 || endpoint.kind == 2) {
                let _ = self.control_transfer(&mut device, 0x21, 0x0a, 0, endpoint.interface as u16, descriptor.virt, descriptor.phys, 0);
                if !self.control_transfer(&mut device, 0x21, 0x0b, 0, endpoint.interface as u16, descriptor.virt, descriptor.phys, 0) {
                    free_dma(descriptor); free_dma(ep0_dma); free_dma(output); return None;
                }
            }
        }
        for (index, value) in found.iter().enumerate() {
            let Some(found_endpoint) = *value else { continue; };
            let ring = match alloc_dma(RING_BYTES) {
                Some(value) => value,
                None => { free_device_memory(&device); free_dma(descriptor); return None; }
            };
            let buffer_len = (found_endpoint.max_packet as usize).max(8).min(1024);
            let buffer = match alloc_dma(buffer_len) {
                Some(value) => value,
                None => { free_dma(ring); free_device_memory(&device); free_dma(descriptor); return None; }
            };
            initialize_ring(ring);
            device.endpoints[index] = Some(Endpoint {
                active: true,
                epid: (found_endpoint.endpoint as u8) * 2 + 1,
                kind: found_endpoint.kind,
                interface: found_endpoint.interface,
                max_packet: found_endpoint.max_packet,
                interval: found_endpoint.interval.max(1).min(16),
                format: found_endpoint.format,
                ring: ring.virt as *mut Trb,
                ring_phys: ring.phys,
                buffer: buffer.virt,
                buffer_phys: buffer.phys,
                buffer_len,
                enqueue: 0,
                cycle: true,
            });
        }
        free_dma(descriptor);
        if device.endpoints.iter().all(|endpoint| endpoint.is_none()) || !self.configure_endpoints(&device) {
            free_device_memory(&device);
            return None;
        }
        for endpoint in device.endpoints.iter_mut().flatten() {
            self.queue_interrupt(endpoint);
            self.doorbell(device.slot, endpoint.epid);
        }
        Some(device)
    }

    fn disable_slot(&mut self, slot: u8) -> bool {
        self.command(0, 0, 0, TRB_TYPE_DISABLE_SLOT | ((slot as u32) << 24), Some(slot))
            .map_or(false, |(code, _)| code == 1)
    }

    fn teardown_slot(&mut self, slot: u8) {
        if slot == 0 || slot as usize > MAX_DEVICES { return; }
        for child in 1..=MAX_DEVICES as u8 {
            if self.devices[child as usize - 1].map_or(false, |device| device.parent_slot == slot) {
                self.teardown_slot(child);
            }
        }
        let index = slot as usize - 1;
        let Some(mut device) = self.devices[index].take() else { return; };
        if !self.disable_slot(slot) {
            for endpoint in device.endpoints.iter_mut().flatten() { endpoint.active = false; }
            self.devices[index] = Some(device);
            println!("xhci: slot {} disable failed", slot);
            return;
        }
        free_device_memory(&device);
        println!("xhci: slot {} removed", slot);
    }

    fn teardown_port(&mut self, port: u8) {
        for slot in 1..=MAX_DEVICES as u8 {
            if self.devices[slot as usize - 1].map_or(false, |device| device.parent_slot == 0 && device.port == port) {
                self.teardown_slot(slot);
            }
        }
    }

    fn store_device(&mut self, device: UsbDevice) {
        let index = device.slot as usize - 1;
        if index < MAX_DEVICES {
            println!("xhci: port {} slot {} ready", device.port, device.slot);
            self.devices[index] = Some(device);
        } else {
            let _ = self.disable_slot(device.slot);
            free_device_memory(&device);
        }
    }

    fn hub_request(&mut self, slot: u8, bm: u8, request: u8, value: u16, index: u16, output: &mut [u8]) -> bool {
        let dma = match alloc_dma(output.len().max(8)) { Some(value) => value, None => return false };
        let device_index = slot as usize;
        if device_index == 0 || device_index > MAX_DEVICES {
            free_dma(dma);
            return false;
        }
        let Some(mut device) = self.devices[device_index - 1] else {
            free_dma(dma);
            return false;
        };
        let result = self.control_transfer(&mut device, bm, request, value, index, dma.virt, dma.phys, output.len() as u16);
        if result && !output.is_empty() {
            for (offset, byte) in output.iter_mut().enumerate() {
                *byte = unsafe { dma.virt.add(offset).read_volatile() };
            }
        }
        self.devices[device_index - 1] = Some(device);
        free_dma(dma);
        result
    }

    fn hub_port_status(&mut self, slot: u8, port: u8) -> Option<u32> {
        let mut status = [0u8; 4];
        if !self.hub_request(slot, 0xa3, 0, 0, port as u16, &mut status) { return None; }
        Some(u32::from_le_bytes(status))
    }

    fn hub_port_speed(status: u32) -> u8 {
        if status & (1 << 10) != 0 { 3 } else if status & (1 << 9) != 0 { 2 } else { 1 }
    }

    fn clear_hub_port_changes(&mut self, slot: u8, port: u8, changes: u16) {
        for (bit, feature) in [(0, 16u16), (1, 17), (2, 18), (3, 19), (4, 20)] {
            if changes & (1 << bit) != 0 {
                let _ = self.hub_request(slot, 0x23, 1, feature, port as u16, &mut []);
            }
        }
    }

    fn reset_hub_port(&mut self, slot: u8, port: u8) -> Option<u8> {
        let _ = self.hub_request(slot, 0x23, 3, 8, port as u16, &mut []);
        sys_sleep(20);
        let status = self.hub_port_status(slot, port)?;
        if status & 1 == 0 { return None; }
        let _ = self.hub_request(slot, 0x23, 3, 4, port as u16, &mut []);
        for _ in 0..100 {
            sys_sleep(10);
            let status = self.hub_port_status(slot, port)?;
            if status & (1 << 4) == 0 {
                self.clear_hub_port_changes(slot, port, (status >> 16) as u16);
                return Some(Self::hub_port_speed(status));
            }
        }
        None
    }

    fn hub_child_route(hub: &UsbDevice, port: u8) -> u32 {
        hub.route | ((port as u32) << (hub.route_depth as u32 * 4))
    }

    fn scan_hub(&mut self, slot: u8) {
        let Some(hub) = self.devices[slot as usize - 1] else { return; };
        for port in 1..=hub.hub_ports {
            let Some(speed) = self.reset_hub_port(slot, port) else { continue; };
            if self.devices.iter().flatten().any(|device| device.parent_slot == slot && device.parent_port == port) { continue; }
            let route = Self::hub_child_route(&hub, port);
            if let Some(device) = self.enumerate_device(hub.port, route, hub.route_depth + 1, slot, port, Some(speed)) {
                let child_slot = device.slot;
                self.store_device(device);
                if self.devices[child_slot as usize - 1].map_or(false, |device| device.hub) {
                    self.scan_hub(child_slot);
                }
            }
        }
    }

    fn hub_port_changed(&mut self, hub_slot: u8, port: u8) {
        let Some(status) = self.hub_port_status(hub_slot, port) else { return; };
        let changes = (status >> 16) as u16;
        self.clear_hub_port_changes(hub_slot, port, changes);
        let child = self.devices.iter().flatten().find_map(|device| {
            if device.parent_slot == hub_slot && device.parent_port == port { Some(device.slot) } else { None }
        });
        if status & 1 == 0 {
            if let Some(slot) = child { self.teardown_slot(slot); }
        } else if child.is_none() {
            let Some(hub) = self.devices[hub_slot as usize - 1] else { return; };
            let Some(speed) = self.reset_hub_port(hub_slot, port) else { return; };
            let route = Self::hub_child_route(&hub, port);
            if let Some(device) = self.enumerate_device(hub.port, route, hub.route_depth + 1, hub_slot, port, Some(speed)) {
                let child_slot = device.slot;
                self.store_device(device);
                if self.devices[child_slot as usize - 1].map_or(false, |device| device.hub) { self.scan_hub(child_slot); }
            }
        }
    }

    fn port_changed(&mut self, port: u8) {
        if port == 0 || port > self.max_ports { return; }
        let status = self.port_read32(port);
        let changes = status & PORT_CHANGE_BITS;
        if changes != 0 { self.port_write32(port, changes); }
        if status & 1 == 0 {
            self.teardown_port(port);
        } else if !self.devices.iter().flatten().any(|device| device.parent_slot == 0 && device.port == port) {
            if let Some(device) = self.enumerate_port(port) {
                let slot = device.slot;
                self.store_device(device);
                if self.devices[slot as usize - 1].map_or(false, |device| device.hub) { self.scan_hub(slot); }
            }
        }
    }

    fn poll(&mut self) {
        for _ in 0..128 {
            let mut pending_port = 0u8;
            for port in 1..=self.max_ports.min(64) {
                if self.pending_ports & (1u64 << (port - 1)) != 0 {
                    self.pending_ports &= !(1u64 << (port - 1));
                    pending_port = port;
                    break;
                }
            }
            if pending_port != 0 {
                self.port_changed(pending_port);
                continue;
            }
            let Some(event) = self.next_event() else { break; };
            if event.kind == EVENT_PORT {
                self.port_changed(event.port);
                continue;
            }
            if event.kind != EVENT_TRANSFER || event.slot == 0 || event.slot as usize > MAX_DEVICES {
                continue;
            }
            let index = event.slot as usize - 1;
            let Some(mut device) = self.devices[index] else { continue; };
            let mut hub_change = None;
            for endpoint in device.endpoints.iter_mut().flatten() {
                if endpoint.epid != event.epid || !endpoint.active { continue; }
                if event.code == 1 || event.code == 13 {
                    let length = endpoint.buffer_len.saturating_sub(event.length.min(endpoint.buffer_len));
                    if endpoint.kind == 1 {
                        send_keyboard(
                            self.keyboard_fd,
                            endpoint.buffer,
                            length,
                            endpoint.format,
                            &mut device.keyboard_previous,
                            &mut device.keyboard_previous_modifiers,
                        );
                    } else if endpoint.kind == 2 {
                        send_mouse(
                            self.mouse_fd,
                            endpoint.format,
                            endpoint.buffer,
                            length,
                            &mut device.mouse_x,
                            &mut device.mouse_y,
                            &mut device.mouse_position_valid,
                        );
                    } else if endpoint.kind == 3 {
                        hub_change = Some((endpoint.buffer, length));
                    }
                }
                self.queue_interrupt(endpoint);
                self.doorbell(device.slot, endpoint.epid);
            }
            self.devices[index] = Some(device);
            if let Some((buffer, length)) = hub_change {
                self.poll_hub_changes(event.slot, buffer, length);
            }
        }
    }

    fn poll_hub_changes(&mut self, hub_slot: u8, buffer: *mut u8, length: usize) {
        let Some(hub) = self.devices[hub_slot as usize - 1] else { return; };
        let max_ports = hub.hub_ports.min(31) as usize;
        for port in 1..=max_ports {
            let byte = port / 8;
            let bit = port % 8;
            if byte < length && unsafe { buffer.add(byte).read_volatile() } & (1 << bit) != 0 {
                self.hub_port_changed(hub_slot, port as u8);
            }
        }
    }

    fn free_resources(&mut self) {
        for device in self.devices.iter().flatten() {
            free_device_memory(device);
        }
        free_dma(self.input);
        free_dma(self.erst);
        free_dma(self.events);
        free_dma(self.command);
        free_dma(self.dcbaa);
    }

    fn abort(&mut self) {
        self.op_write32(0, self.op_read32(0) & !1);
        for _ in 0..100 {
            if self.op_read32(4) & 1 != 0 { break; }
            sys_sleep(1);
        }
        self.free_resources();
    }

    fn shutdown(&mut self) {
        self.abort();
        sys_pci_disable(self.bdf);
        sys_pci_bar_unmap(self.mmio as u64);
        let _ = self.bar_size;
        if self.mouse_fd != u64::MAX { syscall(SYS_CLOSE, self.mouse_fd, 0, 0); }
        if self.keyboard_fd != u64::MAX { syscall(SYS_CLOSE, self.keyboard_fd, 0, 0); }
    }
}

fn allocate_controller_dma() -> Option<(Dma, Dma, Dma, Dma, Dma)> {
    let dcbaa = alloc_dma(4096)?;
    let command = match alloc_dma(RING_BYTES) {
        Some(value) => value,
        None => { free_dma(dcbaa); return None; }
    };
    let events = match alloc_dma(RING_BYTES) {
        Some(value) => value,
        None => { free_dma(command); free_dma(dcbaa); return None; }
    };
    let erst = match alloc_dma(4096) {
        Some(value) => value,
        None => { free_dma(events); free_dma(command); free_dma(dcbaa); return None; }
    };
    let input = match alloc_dma(4096) {
        Some(value) => value,
        None => { free_dma(erst); free_dma(events); free_dma(command); free_dma(dcbaa); return None; }
    };
    Some((dcbaa, command, events, erst, input))
}

fn initial_mps(speed: u8) -> u16 {
    match speed {
        3 => 64,
        4 | 5 => 512,
        _ => 8,
    }
}

fn hid_local_usage(usages: &[u16; 8], usage_count: usize, usage_min: Option<u16>, usage_max: Option<u16>, index: usize) -> Option<u16> {
    if index < usage_count {
        Some(usages[index])
    } else if let (Some(min), Some(max)) = (usage_min, usage_max) {
        let usage = min.checked_add(index as u16)?;
        if usage <= max { Some(usage) } else { None }
    } else {
        None
    }
}

fn hid_report_format(base: *mut u8, total: usize) -> Option<HidFormat> {
    let mut format = HidFormat::empty();
    let mut offset = 0usize;
    let mut usage_page = 0u16;
    let mut report_id = 0u8;
    let mut report_size = 0u32;
    let mut report_count = 0u32;
    let mut logical_min = 0i32;
    let mut logical_max = 0i32;
    let mut usages = [0u16; 8];
    let mut usage_count = 0usize;
    let mut usage_min = None;
    let mut usage_max = None;
    let mut collection_depth = 0u8;
    let mut application_kind = 0u8;
    let mut descriptor_kind = 0u8;
    let mut selected_report_id = None;
    let mut report_offset = 0u32;

    while offset < total {
        let prefix = unsafe { base.add(offset).read_volatile() };
        if prefix == 0xfe {
            if offset + 2 >= total { return None; }
            let length = unsafe { base.add(offset + 1).read_volatile() } as usize;
            if offset.checked_add(3)?.checked_add(length)? > total { return None; }
            offset += 3 + length;
            continue;
        }
        let size = match prefix & 3 { 0 => 0, 1 => 1, 2 => 2, _ => 4 };
        if offset.checked_add(1)?.checked_add(size)? > total { return None; }
        let mut value = 0u32;
        for index in 0..size {
            value |= (unsafe { base.add(offset + 1 + index).read_volatile() } as u32) << (index * 8);
        }
        let tag = prefix & 0xfc;
        match tag {
            0x04 => usage_page = value as u16,
            0x08 => {
                if usage_count < usages.len() { usages[usage_count] = value as u16; }
                usage_count = usage_count.saturating_add(1);
            }
            0x18 => usage_min = Some(value as u16),
            0x28 => usage_max = Some(value as u16),
            0x14 => logical_min = hid_signed_item(value, size),
            0x24 => logical_max = hid_signed_item(value, size),
            0x74 => report_size = value,
            0x84 => {
                if size != 1 || value == 0 { return None; }
                report_id = value as u8;
            }
            0x94 => report_count = value,
            0xa0 => {
                if collection_depth == 0 && (value & 0xff) == 1 {
                    let usage = if usage_count != 0 { Some(usages[0]) } else { usage_min };
                    let kind = match (usage_page, usage) {
                        (1, Some(6)) => 1,
                        (1, Some(2)) => 2,
                        _ => 0,
                    };
                    if kind != 0 {
                        if descriptor_kind != 0 && descriptor_kind != kind { return None; }
                        descriptor_kind = kind;
                    }
                    application_kind = kind;
                }
                collection_depth = collection_depth.checked_add(1)?;
                if collection_depth > 8 { return None; }
                usage_count = 0;
                usage_min = None;
                usage_max = None;
            }
            0xc0 => {
                if collection_depth == 0 { return None; }
                collection_depth -= 1;
                if collection_depth == 0 { application_kind = 0; }
                usage_count = 0;
                usage_min = None;
                usage_max = None;
            }
            0x80 => {
                if report_size == 0 || report_count == 0 || report_count > 255 || report_size > 32 {
                    return None;
                }
                if selected_report_id.is_none() {
                    selected_report_id = Some(report_id);
                } else if selected_report_id != Some(report_id) {
                    return None;
                }
                let field_bits = report_size.checked_mul(report_count)?;
                let field_end = report_offset.checked_add(field_bits)?;
                if field_end > u16::MAX as u32 { return None; }
                let constant = value & 1 != 0;
                let variable = value & 2 != 0;
                let absolute = value & 4 == 0;
                let field_offset = report_offset;
                if !constant && application_kind == 2 && usage_page == 9 && variable && report_size == 1 {
                    for index in 0..report_count as usize {
                        let Some(usage) = hid_local_usage(&usages, usage_count.min(usages.len()), usage_min, usage_max, index) else { continue; };
                        if (1..=3).contains(&usage) {
                            let button = (usage - 1) as usize;
                            if format.button_offsets[button] != HID_OFFSET_NONE { return None; }
                            format.button_offsets[button] = (field_offset + index as u32) as u16;
                        }
                    }
                }
                if !constant && application_kind == 2 && usage_page == 1 && variable && (report_size == 8 || report_size == 16) {
                    for index in 0..report_count as usize {
                        let Some(usage) = hid_local_usage(&usages, usage_count.min(usages.len()), usage_min, usage_max, index) else { continue; };
                        let item_offset = (field_offset + index as u32 * report_size) as u16;
                        if usage == 0x30 {
                            if format.x_offset != HID_OFFSET_NONE { return None; }
                            format.x_offset = item_offset;
                            format.x_size = report_size as u8;
                            format.x_absolute = absolute;
                            format.x_logical_min = logical_min;
                            format.x_logical_max = logical_max;
                        } else if usage == 0x31 {
                            if format.y_offset != HID_OFFSET_NONE { return None; }
                            format.y_offset = item_offset;
                            format.y_size = report_size as u8;
                            format.y_absolute = absolute;
                            format.y_logical_min = logical_min;
                            format.y_logical_max = logical_max;
                        }
                    }
                }
                if !constant && application_kind == 1 && usage_page == 7 && variable && report_size == 1 && report_count == 8 && usage_min == Some(0xe0) && usage_max == Some(0xe7) {
                    if format.modifier_offset != HID_OFFSET_NONE { return None; }
                    format.modifier_offset = field_offset as u16;
                }
                if !constant && application_kind == 1 && usage_page == 7 && !variable && report_size == 8 && (1..=8).contains(&report_count) && usage_min.is_some() && usage_max.is_some() {
                    if format.key_offset != HID_OFFSET_NONE { return None; }
                    format.key_offset = field_offset as u16;
                    format.key_count = report_count as u8;
                }
                report_offset = field_end;
                usage_count = 0;
                usage_min = None;
                usage_max = None;
            }
            _ => {}
        }
        offset += 1 + size;
    }

    if collection_depth != 0 || descriptor_kind == 0 || selected_report_id.is_none() || report_offset == 0 || report_offset > u16::MAX as u32 {
        return None;
    }
    format.kind = descriptor_kind;
    format.report_id = selected_report_id?;
    format.report_bits = report_offset as u16;
    if format.kind == 2 {
        if format.button_offsets.iter().any(|offset| *offset == HID_OFFSET_NONE)
            || format.x_offset == HID_OFFSET_NONE
            || format.y_offset == HID_OFFSET_NONE
            || format.x_absolute != format.y_absolute
            || format.x_logical_min > format.x_logical_max
            || format.y_logical_min > format.y_logical_max
            || (format.x_absolute && (format.x_logical_min < 0 || format.y_logical_min < 0)) {
            return None;
        }
    } else if format.modifier_offset == HID_OFFSET_NONE || format.key_offset == HID_OFFSET_NONE || format.key_count == 0 {
        return None;
    }
    Some(format)
}

fn hid_signed_item(value: u32, size: usize) -> i32 {
    match size {
        1 => value as u8 as i8 as i32,
        2 => value as u16 as i16 as i32,
        _ => value as i32,
    }
}

fn parse_hid_endpoints(base: *mut u8, total: usize, found: &mut [Option<FoundEndpoint>; MAX_ENDPOINTS]) {
    let mut offset = 0usize;
    let mut kind = 0u8;
    let mut interface = 0u8;
    let mut subclass = 0u8;
    let mut report_length = 0u16;
    while offset + 2 <= total {
        let length = unsafe { base.add(offset).read_volatile() } as usize;
        let dtype = unsafe { base.add(offset + 1).read_volatile() };
        if length < 2 || offset + length > total { break; }
        if dtype == 4 && length >= 9 {
            let class = unsafe { base.add(offset + 5).read_volatile() };
            interface = unsafe { base.add(offset + 2).read_volatile() };
            subclass = unsafe { base.add(offset + 6).read_volatile() };
            report_length = 0;
            kind = if class == 3 { 4 } else if class == 9 { 3 } else { 0 };
        } else if dtype == 0x21 && length >= 9 && kind == 4 {
            report_length = unsafe { u16::from_le_bytes([base.add(offset + 7).read_volatile(), base.add(offset + 8).read_volatile()]) };
        } else if dtype == 5 && length >= 7 && kind != 0 {
            let address = unsafe { base.add(offset + 2).read_volatile() };
            let attributes = unsafe { base.add(offset + 3).read_volatile() };
            let max_packet = unsafe { u16::from_le_bytes([base.add(offset + 4).read_volatile(), base.add(offset + 5).read_volatile()]) } & 0x07ff;
            let interval = unsafe { base.add(offset + 6).read_volatile() };
            if address & 0x80 != 0 && attributes & 3 == 3 && address & 0x0f != 0 && max_packet != 0 {
                let index = if kind == 3 || kind == 4 { 0 } else { 1 };
                if found[index].is_none() {
                    found[index] = Some(FoundEndpoint {
                        kind,
                        interface,
                        endpoint: address & 0x0f,
                        max_packet,
                        interval,
                        report_length,
                        subclass,
                        format: HidFormat::empty(),
                    });
                }
            }
        }
        offset += length;
    }
}

fn free_device_memory(device: &UsbDevice) {
    if !device.ep0_ring.is_null() { sys_dma_free(device.ep0_ring as *mut u8); }
    for endpoint in device.endpoints.iter().flatten() {
        if !endpoint.ring.is_null() { sys_dma_free(endpoint.ring as *mut u8); }
        if !endpoint.buffer.is_null() { sys_dma_free(endpoint.buffer); }
    }
    if !device.output.is_null() { sys_dma_free(device.output); }
}

fn hid_read_bits(base: *mut u8, length: usize, offset: u16, size: u8) -> Option<u32> {
    if size == 0 || size > 32 { return None; }
    let start = offset as usize;
    let end = start.checked_add(size as usize)?;
    if end > length.checked_mul(8)? { return None; }
    let mut value = 0u32;
    for bit in 0..size as usize {
        let byte = unsafe { base.add((start + bit) / 8).read_volatile() };
        value |= (((byte >> ((start + bit) % 8)) & 1) as u32) << bit;
    }
    Some(value)
}

fn hid_report_value(value: u32, size: u8, logical_min: i32) -> i32 {
    if logical_min < 0 {
        match size {
            8 => value as u8 as i8 as i32,
            16 => value as u16 as i16 as i32,
            _ => value as i32,
        }
    } else {
        value as i32
    }
}


fn send_mouse(
    mouse_fd: u64,
    format: HidFormat,
    buffer: *mut u8,
    length: usize,
    previous_x: &mut i32,
    previous_y: &mut i32,
    position_valid: &mut bool,
) {
    if format.report_id != 0 && (length == 0 || unsafe { buffer.read_volatile() } != format.report_id) { return; }
    let report_bytes = (format.report_bits as usize + 7) / 8 + usize::from(format.report_id != 0);
    if length < report_bytes { return; }
    let payload_offset: u16 = if format.report_id == 0 { 0 } else { 8 };
    let Some(x_offset) = format.x_offset.checked_add(payload_offset) else { return; };
    let Some(y_offset) = format.y_offset.checked_add(payload_offset) else { return; };
    let Some(x_raw) = hid_read_bits(buffer, length, x_offset, format.x_size) else { return; };
    let Some(y_raw) = hid_read_bits(buffer, length, y_offset, format.y_size) else { return; };
    let x = hid_report_value(x_raw, format.x_size, format.x_logical_min);
    let y = hid_report_value(y_raw, format.y_size, format.y_logical_min);
    if x < format.x_logical_min || x > format.x_logical_max || y < format.y_logical_min || y > format.y_logical_max { return; }
    let mut buttons = 0u8;
    for (index, button_offset) in format.button_offsets.iter().enumerate() {
        let Some(offset) = button_offset.checked_add(payload_offset) else { return; };
        let Some(value) = hid_read_bits(buffer, length, offset, 1) else { return; };
        buttons |= (value as u8) << index;
    }
    let (dx, dy) = if format.x_absolute {
        let dx = if *position_valid { x - *previous_x } else { 0 };
        let dy = if *position_valid { -(y - *previous_y) } else { 0 };
        *previous_x = x;
        *previous_y = y;
        *position_valid = true;
        (dx, dy)
    } else {
        (x, -y)
    };
    // Write one record to /dev/input/mouse; the kernel queue merges all drivers.
    if mouse_fd == u64::MAX { return; }
    let mut record = [0u8; 12];
    record[0] = buttons;
    record[4..8].copy_from_slice(&dx.to_le_bytes());
    record[8..12].copy_from_slice(&dy.to_le_bytes());
    syscall(SYS_WRITE, mouse_fd, record.as_ptr() as u64, record.len() as u64);
}

fn hid_char(usage: u8, shift: bool) -> Option<u8> {
    match usage {
        4..=29 => Some(if shift { b'A' + usage - 4 } else { b'a' + usage - 4 }),
        30..=39 => Some(if shift { b"!@#$%^&*()"[(usage - 30) as usize] } else { b"1234567890"[(usage - 30) as usize] }),
        40 => Some(b'\n'), 41 => Some(27), 42 => Some(8), 43 => Some(b'\t'), 44 => Some(b' '),
        45..=56 => {
            let index = (usage - 45) as usize;
            Some(if shift { b"_+{}|~:\"~<>?".get(index).copied().unwrap_or(0) } else { b"-=[]\\#;'`,./".get(index).copied().unwrap_or(0) })
        }
        79 => Some(0x81), 80 => Some(0x80), 81 => Some(0x83), 82 => Some(0x82),
        _ => None,
    }.filter(|ch| *ch != 0)
}

fn hid_key_code(usage: u8, shift: bool, ctrl: bool) -> Option<u8> {
    let code = match usage {
        0xe0 => 0x86,
        0xe1 => 0x84,
        0xe2 => 0x88,
        0xe4 => 0x87,
        0xe5 => 0x85,
        0xe6 => 0x89,
        0xe3 | 0xe7 => return None,
        0x29 => 0x8b,
        0x39 => 0x8a,
        0x3a..=0x45 => 0x90 + usage - 0x3a,
        _ => {
            let mut code = hid_char(usage, shift)?;
            if ctrl && code.is_ascii_alphabetic() { code = code.to_ascii_lowercase() - b'a' + 1; }
            code
        }
    };
    Some(code)
}

fn send_keyboard_event(keyboard_fd: u64, usage: u8, release: bool, modifiers: u8, key_modifiers: u8) {
    let shift = modifiers & 0x22 != 0;
    let ctrl = modifiers & 0x11 != 0;
    let code = hid_key_code(usage, key_modifiers & 0x22 != 0, key_modifiers & 0x11 != 0);
    let Some(code) = code else { return; };
    let mut event = code as u16;
    if release { event |= KEY_RELEASE; }
    if shift { event |= MOD_SHIFT; }
    if ctrl { event |= MOD_CTRL; }
    if modifiers & 0x44 != 0 { event |= MOD_ALT; }
    if keyboard_fd != u64::MAX {
        let record = event.to_le_bytes();
        syscall(SYS_WRITE, keyboard_fd, record.as_ptr() as u64, record.len() as u64);
    }
}

fn send_keyboard(
    keyboard_fd: u64,
    buffer: *mut u8,
    length: usize,
    format: HidFormat,
    previous: &mut [u8; 8],
    previous_modifiers: &mut u8,
) {
    if format.report_id != 0 && (length == 0 || unsafe { buffer.read_volatile() } != format.report_id) { return; }
    let report_bytes = (format.report_bits as usize + 7) / 8 + usize::from(format.report_id != 0);
    if length < report_bytes { return; }
    let payload_offset: u16 = if format.report_id == 0 { 0 } else { 8 };
    let Some(modifier_offset) = format.modifier_offset.checked_add(payload_offset) else { return; };
    let Some(modifiers) = hid_read_bits(buffer, length, modifier_offset, 8) else { return; };
    let modifiers = modifiers as u8;
    let mut current = [0u8; 8];
    for index in 0..format.key_count as usize {
        let bit_offset = match format.key_offset.checked_add((index * 8) as u16).and_then(|offset| offset.checked_add(payload_offset)) {
            Some(offset) => offset,
            None => return,
        };
        let Some(key) = hid_read_bits(buffer, length, bit_offset, 8) else { return; };
        current[index] = key as u8;
    }
    if current[..format.key_count as usize].iter().any(|usage| (1..=3).contains(usage)) { return; }
    let current_keys = &current[..format.key_count as usize];
    let previous_keys = &previous[..format.key_count as usize];
    for bit in 0..8 {
        let mask = 1u8 << bit;
        if (*previous_modifiers ^ modifiers as u8) & mask != 0 {
            send_keyboard_event(keyboard_fd, 0xe0 + bit, modifiers & mask == 0, modifiers as u8, *previous_modifiers);
        }
    }
    for usage in previous_keys.iter().copied().filter(|usage| *usage != 0) {
        if !current_keys.contains(&usage) { send_keyboard_event(keyboard_fd, usage, true, modifiers as u8, *previous_modifiers); }
    }
    for usage in current_keys.iter().copied().filter(|usage| *usage != 0) {
        if !previous_keys.contains(&usage) { send_keyboard_event(keyboard_fd, usage, false, modifiers as u8, modifiers as u8); }
    }
    *previous = current;
    *previous_modifiers = modifiers as u8;
}

fn controller_init(bdf: u32, mmio: u64, bar_size: u64) -> Option<Controller> {
    let mmio_ptr = mmio as *mut u8;
    let cap_length = unsafe { mmio_ptr.read_volatile() } as u32;
    let hcs_params1 = unsafe { mmio_ptr.add(4).cast::<u32>().read_volatile() };
    let hcc_params1 = unsafe { mmio_ptr.add(0x10).cast::<u32>().read_volatile() };
    let doorbell = unsafe { mmio_ptr.add(0x14).cast::<u32>().read_volatile() } & !3;
    let runtime = unsafe { mmio_ptr.add(0x18).cast::<u32>().read_volatile() } & !0x1f;
    let max_slots = (hcs_params1 & 0xff).min(MAX_DEVICES as u32) as u8;
    let max_ports = ((hcs_params1 >> 24) & 0xff) as u8;
    if cap_length < 0x20 || doorbell == 0 || runtime == 0 || max_slots == 0 || max_ports == 0 || bar_size < runtime as u64 + 0x400 {
        return None;
    }
    let (dcbaa, command, events, erst, input) = allocate_controller_dma()?;
    initialize_ring(command);
    unsafe {
        let table = erst.virt as *mut u32;
        table.add(0).write_volatile(events.phys as u32);
        table.add(1).write_volatile((events.phys >> 32) as u32);
        table.add(2).write_volatile(RING_TRBS as u32);
        table.add(3).write_volatile(0);
    }
    let mut controller = Controller {
        bdf,
        mmio: mmio_ptr,
        bar_size,
        op: cap_length,
        runtime,
        doorbell,
        max_ports,
        context_size: if hcc_params1 & (1 << 2) != 0 { 64 } else { 32 },
        max_slots,
        dcbaa,
        command,
        events,
        erst,
        input,
        command_enqueue: 0,
        command_cycle: true,
        event_dequeue: 0,
        event_cycle: true,
        pending_ports: 0,
        devices: [None; MAX_DEVICES],
        mouse_fd: u64::MAX,
        keyboard_fd: u64::MAX,
    };
    if !controller.claim_legacy(hcc_params1) {
        controller.abort();
        return None;
    }
    let command_reg = controller.op_read32(0);
    controller.op_write32(0, command_reg & !1);
    for _ in 0..500 {
        if controller.op_read32(4) & 1 != 0 { break; }
        sys_sleep(1);
    }
    controller.op_write32(0, controller.op_read32(0) | 2);
    for _ in 0..500 {
        if controller.op_read32(0) & 2 == 0 && controller.op_read32(4) & (1 << 11) == 0 { break; }
        sys_sleep(1);
    }
    if controller.op_read32(0) & 2 != 0 || controller.op_read32(4) & (1 << 11) != 0 {
        controller.abort();
        return None;
    }
    controller.op_write32(0x30, controller.dcbaa.phys as u32);
    controller.op_write32(0x34, (controller.dcbaa.phys >> 32) as u32);
    controller.op_write32(0x18, controller.command.phys as u32 | 1);
    controller.op_write32(0x1c, (controller.command.phys >> 32) as u32);
    controller.op_write32(0x38, controller.max_slots as u32);
    controller.runtime_write32(0x20, 0);
    controller.runtime_write32(0x24, 0);
    controller.runtime_write32(0x28, 1);
    controller.runtime_write32(0x30, controller.erst.phys as u32);
    controller.runtime_write32(0x34, (controller.erst.phys >> 32) as u32);
    controller.runtime_write32(0x38, controller.events.phys as u32 | 8);
    controller.write32(controller.runtime + 0x3c, (controller.events.phys >> 32) as u32);
    controller.op_write32(0, controller.op_read32(0) | 1);
    for _ in 0..500 {
        if controller.op_read32(4) & 1 == 0 { break; }
        sys_sleep(1);
    }
    if controller.op_read32(4) & 1 != 0 {
        controller.abort();
        return None;
    }
    Some(controller)
}

pub fn kdm_init() -> bool {
    let Some(bdf) = find_controller() else {
        println!("xhci: no controller");
        return false;
    };
    let mut bar_size = 0u64;
    let mmio = sys_pci_bar_map_with_size(bdf, 0, Some(&mut bar_size));
    if mmio == u64::MAX { println!("xhci: BAR map failed"); return false; }
    println!("xhci: BAR size {}", bar_size);
    if sys_pci_enable(bdf) == u64::MAX {
        sys_pci_bar_unmap(mmio);
        return false;
    }
    let Some(mut controller) = controller_init(bdf, mmio, bar_size) else {
        println!("xhci: controller init failed");
        sys_pci_disable(bdf);
        sys_pci_bar_unmap(mmio);
        return false;
    };
    controller.mouse_fd = syscall(SYS_OPEN, b"/dev/input/mouse".as_ptr() as u64, 16, 0);
    if controller.mouse_fd == u64::MAX {
        controller.shutdown();
        return false;
    }
    controller.keyboard_fd = syscall(SYS_OPEN, b"/dev/input/keyboard".as_ptr() as u64, 19, 0);
    if controller.keyboard_fd == u64::MAX {
        controller.shutdown();
        return false;
    }
    println!("xhci: controller ready, scanning {} ports", controller.max_ports);
    for port in 1..=controller.max_ports {
        if let Some(device) = controller.enumerate_port(port) {
            let slot = device.slot;
            controller.store_device(device);
            if controller.devices[slot as usize - 1].map_or(false, |device| device.hub) {
                controller.scan_hub(slot);
            }
        }
    }
    unsafe {
        let state = core::ptr::addr_of_mut!(CONTROLLER);
        core::ptr::write(state, Some(controller));
    }
    true
}

pub fn kdm_run() {
    let mut port_poll_ticks = 0u8;
    loop {
        unsafe {
            let state = core::ptr::addr_of_mut!(CONTROLLER);
            let Some(controller) = (&mut *state).as_mut() else { return; };
            controller.poll();
            port_poll_ticks = port_poll_ticks.wrapping_add(1);
            if port_poll_ticks >= 20 {
                port_poll_ticks = 0;
                for port in 1..=controller.max_ports {
                    let connected = controller.port_read32(port) & 1 != 0;
                    let known = controller.devices.iter().flatten().any(|device| device.parent_slot == 0 && device.port == port);
                    if connected != known { controller.port_changed(port); }
                }
            }
        }
        if sys_signal_check() { return; }
        sys_sleep_tick();
    }
}

pub fn kdm_exit() {
    unsafe {
        let state = core::ptr::addr_of_mut!(CONTROLLER);
        let controller = core::ptr::read(state);
        core::ptr::write(state, None);
        if let Some(mut controller) = controller {
            controller.shutdown();
        }
    }
}

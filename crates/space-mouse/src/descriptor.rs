//! Just enough of a HID report descriptor reader to recognise a 3D mouse and to find its six
//! motion axes and its buttons in its input reports. Written from the public USB HID
//! specification (Device Class Definition for HID 1.11, section 6.2.2, and the HID Usage Tables).
//! Malformed input never panics; it simply yields less information.

/// Generic Desktop usage page, and the usages in it that matter here.
const PAGE_GENERIC_DESKTOP: u32 = 0x01;
const USAGE_MULTI_AXIS_CONTROLLER: u32 = 0x08;
const FIRST_AXIS_USAGE: u32 = 0x30; // X, Y, Z, Rx, Ry, Rz are 0x30..=0x35
const AXIS_COUNT: usize = 6;
/// The Button usage page: usage N is button N, from 1.
const PAGE_BUTTON: u32 = 0x09;
/// Buttons 1 to this are read; [`Report::buttons`] has one bit for each.
pub const MAX_BUTTONS: usize = 32;

/// Descriptors are small (hidapi reserves 4 KiB). Anything past this is ignored.
const MAX_DESCRIPTOR_BYTES: usize = 4096;
/// Bound on the usages one main item can declare through a usage range, and on its fields.
const MAX_USAGES_PER_ITEM: u32 = 256;
/// Bound on a report's length in bits: hidraw reads at most 4096 bytes.
const MAX_REPORT_BITS: u32 = 4096 * 8;

/// What decoded axes are scaled to at full deflection: 3Dconnexion's own range, which the rest
/// of the 3D mouse code is tuned for. A device declaring ±350 is passed through unchanged.
pub const FULL_SCALE: i16 = 350;

/// Where one axis or button sits in the device's input reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// The report that carries it; 0 when the device doesn't number its reports.
    pub report_id: u8,
    /// Its first bit in the report, not counting the report id byte.
    pub bit_offset: u32,
    /// Its width, 1 to 32 bits. A field of any other width reads as missing.
    pub bits: u32,
    /// The declared range. 64 bits wide so a 32-bit unsigned maximum fits.
    pub logical_min: i64,
    pub logical_max: i64,
    /// The descriptor's Relative flag. 3Dconnexion sets it on some spring-return caps and not on
    /// others, so it does not tell a joystick from a trackball.
    pub relative: bool,
}

impl Field {
    /// The `index`th signed 16-bit value of a report, ±350: how 3Dconnexion devices report.
    const fn sixteen_bit(report_id: u8, index: u32) -> Self {
        Self { report_id, bit_offset: index * 16, bits: 16, logical_min: -350, logical_max: 350, relative: true }
    }

    /// The field's raw value in `body` (the report without its id), or None if `body` is too short
    /// or the field isn't 1 to 32 bits wide. The fields are public, so a hand-built one is checked
    /// here too, not only the ones `Descriptor::parse` makes.
    fn read(&self, body: &[u8]) -> Option<i64> {
        if !(1..=32).contains(&self.bits) {
            return None;
        }
        let mut raw = 0_i64;
        for bit in 0..self.bits {
            let at = self.bit_offset.checked_add(bit)?;
            let byte = *body.get(usize::try_from(at / 8).ok()?)?;
            raw |= i64::from((byte >> (at % 8)) & 1) << bit;
        }
        // Signed fields are two's complement, as the logical minimum says.
        Some(if self.logical_min < 0 && (raw >> (self.bits - 1)) & 1 == 1 { raw - (1_i64 << self.bits) } else { raw })
    }

    /// `raw` scaled from the field's logical range so its middle is 0 and its ends are
    /// ±[`FULL_SCALE`]. A value past the declared range goes past ±FULL_SCALE rather than being cut
    /// off, and a field without a usable range is passed through. Either way the result is clamped
    /// to ±`i16::MAX`, never `i16::MIN`, so negating an axis can't overflow.
    fn scaled(&self, raw: i64) -> i16 {
        // i128: a hand-built Field can declare any i64 range, and `max - min` of i64::MIN..i64::MAX
        // overflows an i64.
        let (raw, min, max) = (i128::from(raw), i128::from(self.logical_min), i128::from(self.logical_max));
        let value = if min < max { (2 * raw - (min + max)) as f64 / (max - min) as f64 * f64::from(FULL_SCALE) } else { raw as f64 };
        value.round().clamp(-f64::from(i16::MAX), f64::from(i16::MAX)) as i16
    }
}

/// Where a device puts its six axes and its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// X, Y, Z, Rx, Ry, Rz, for each axis the device has.
    pub axes: [Option<Field>; AXIS_COUNT],
    /// Buttons 1 to [`MAX_BUTTONS`] at indices 0 to 31, for each the device has. Devices number
    /// them with gaps (3Dconnexion's do), so the index is the button's own number less one.
    pub buttons: [Option<Field>; MAX_BUTTONS],
    /// The descriptor declares report ids, so every report starts with its id byte.
    pub numbered: bool,
}

impl Default for Layout {
    /// 3Dconnexion's: report 1 carries X Y Z and report 2 Rx Ry Rz, three signed 16-bit values
    /// each. For a 3Dconnexion device whose descriptor can't be read; a device that isn't one
    /// should never be read with it. Where its buttons are differs from model to model, so it has
    /// none.
    fn default() -> Self {
        let translation = |index| Some(Field::sixteen_bit(1, index));
        let rotation = |index| Some(Field::sixteen_bit(2, index));
        Self {
            axes: [translation(0), translation(1), translation(2), rotation(0), rotation(1), rotation(2)],
            buttons: [None; MAX_BUTTONS],
            numbered: true,
        }
    }
}

/// What one input report carries: the axes it has, scaled to ±[`FULL_SCALE`], and the buttons.
/// An axis the report doesn't carry is None, not 0, so a device that spreads one group over
/// several reports (X in one, Y in another) doesn't zero the axes it sent a moment ago.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// X, Y, Z, Rx, Ry, Rz.
    pub axes: [Option<i16>; AXIS_COUNT],
    /// When the report carries buttons, which are down: bit N for button N + 1. A report with
    /// every button up is how a device says they were released, so `Some(0)` matters.
    pub buttons: Option<u32>,
}

impl Layout {
    /// The axes and buttons in one input report, or None when it carries none of them or is too
    /// short for one of its fields. A truncated report is dropped whole, axes included, rather
    /// than half read: devices send each report at its full declared size, so a short one is
    /// damaged. Padding after the fields is ignored.
    pub fn decode(&self, report: &[u8]) -> Option<Report> {
        let (id, body) = if self.numbered { (*report.first()?, report.get(1..)?) } else { (0, report) };
        let mut decoded = Report::default();
        for (value, field) in decoded.axes.iter_mut().zip(&self.axes) {
            if let Some(field) = field.filter(|field| field.report_id == id) {
                *value = Some(field.scaled(field.read(body)?));
            }
        }
        for (bit, field) in self.buttons.iter().enumerate() {
            if let Some(field) = field.filter(|field| field.report_id == id) {
                let down = field.read(body)? != 0;
                decoded.buttons = Some(decoded.buttons.unwrap_or(0) | (u32::from(down) << bit));
            }
        }
        (decoded != Report::default()).then_some(decoded)
    }
}

#[derive(Clone, Copy)]
struct Item {
    /// 0 main, 1 global, 2 local (3 is reserved).
    kind: u8,
    tag: u8,
    /// Data bytes: 0, 1, 2 or 4.
    size: u8,
    value: u32,
}

impl Item {
    /// The data as a two's complement number of its own size (logical minimum and maximum).
    fn signed(self) -> i64 {
        match self.size {
            1 => i64::from(self.value as u8 as i8),
            2 => i64::from(self.value as u16 as i16),
            _ => i64::from(self.value as i32),
        }
    }
}

/// Walks the short items of a descriptor. Long items are skipped; a truncated tail ends the walk.
fn items(descriptor: &[u8]) -> impl Iterator<Item = Item> + '_ {
    let descriptor = descriptor.get(..MAX_DESCRIPTOR_BYTES).unwrap_or(descriptor);
    let mut pos = 0_usize;
    std::iter::from_fn(move || {
        loop {
            let prefix = *descriptor.get(pos)?;
            pos += 1;
            if prefix == 0xFE {
                // Long item: a data length byte, a tag byte, then the data.
                let length = usize::from(*descriptor.get(pos)?);
                pos = pos.checked_add(2 + length)?;
                continue;
            }
            let size = match prefix & 0b11 {
                0 => 0,
                1 => 1,
                2 => 2,
                _ => 4,
            };
            let data = descriptor.get(pos..pos + size)?;
            pos += size;
            let value = data.iter().enumerate().fold(0_u32, |value, (index, byte)| value | (u32::from(*byte) << (8 * index)));
            return Some(Item { kind: (prefix >> 2) & 0b11, tag: prefix >> 4, size: size as u8, value });
        }
    })
}

/// The global items in force (PUSH and POP save and restore them).
#[derive(Clone, Copy, Default)]
struct Globals {
    page: u32,
    logical_min: i64,
    logical_max: Option<Item>,
    report_size: u32,
    report_count: u32,
    report_id: u8,
}

impl Globals {
    /// The logical maximum. With a non-negative minimum it is unsigned, so `0x26 0xFF 0xFF` is
    /// 65535 and `0x27 0xFF 0xFF 0xFF 0xFF` is 4294967295, rather than -1, as devices mean it.
    fn logical_max(&self) -> i64 {
        let Some(item) = self.logical_max else { return 0 };
        let signed = item.signed();
        if self.logical_min >= 0 && signed < 0 { i64::from(item.value) } else { signed }
    }
}

/// What a descriptor says about a device, as far as 3D mouse support needs to know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    multi_axis_controller: bool,
    axes: [Option<Field>; AXIS_COUNT],
    buttons: [Option<Field>; MAX_BUTTONS],
    numbered: bool,
}

impl Descriptor {
    pub fn parse(descriptor: &[u8]) -> Self {
        let mut summary = Self { multi_axis_controller: false, axes: [None; AXIS_COUNT], buttons: [None; MAX_BUTTONS], numbered: false };
        let mut globals = Globals::default();
        // Every PUSH is kept, so every POP restores what its own PUSH saved. The descriptor is cut
        // at MAX_DESCRIPTOR_BYTES, which bounds the stack at that many entries.
        let mut pushed: Vec<Globals> = Vec::new();
        let mut depth = 0_u32;
        // The depth of the open Multi-axis Controller application collection; its fields count.
        let mut controller: Option<u32> = None;
        // The next free input bit of each report id.
        let mut offsets: Vec<(u8, u32)> = Vec::new();
        // Local items belong to the next main item only.
        let mut usages: Vec<u32> = Vec::new();
        let mut usage_min = None;
        for item in items(descriptor) {
            match (item.kind, item.tag) {
                // Global: usage page, logical extent, report size, id and count, push, pop.
                (1, 0) => globals.page = item.value,
                (1, 1) => globals.logical_min = item.signed(),
                (1, 2) => globals.logical_max = Some(item),
                (1, 7) => globals.report_size = item.value,
                (1, 8) => {
                    globals.report_id = u8::try_from(item.value).unwrap_or(0);
                    summary.numbered = true;
                }
                (1, 9) => globals.report_count = item.value,
                (1, 0xA) => pushed.push(globals),
                (1, 0xB) => {
                    if let Some(saved) = pushed.pop() {
                        globals = saved;
                    }
                }
                // Local: usage, usage minimum, usage maximum.
                (2, 0) => usages.push(full_usage(globals.page, item)),
                (2, 1) => usage_min = Some(full_usage(globals.page, item)),
                (2, 2) => {
                    if let Some(min) = usage_min.take() {
                        let max = full_usage(globals.page, item);
                        let count = max.saturating_sub(min).min(MAX_USAGES_PER_ITEM - 1);
                        usages.extend((0..=count).map(|offset| min.saturating_add(offset)));
                    }
                }
                // Main: input, collection, end collection.
                (0, 8) => {
                    let start = match offsets.iter().position(|(id, _)| *id == globals.report_id) {
                        Some(index) => offsets.get(index).map_or(0, |(_, bits)| *bits),
                        None => {
                            offsets.push((globals.report_id, 0));
                            0
                        }
                    };
                    // Data (not constant) and variable (not an array): one value per usage.
                    if controller.is_some() && item.value & 0b11 == 0b10 {
                        summary.note_fields(&globals, &usages, start, item.value & 0b100 != 0);
                    }
                    let end = start.saturating_add(globals.report_size.saturating_mul(globals.report_count)).min(MAX_REPORT_BITS);
                    if let Some((_, bits)) = offsets.iter_mut().find(|(id, _)| *id == globals.report_id) {
                        *bits = end;
                    }
                }
                (0, 0xA) => {
                    // An application collection (1) at the top level names the device.
                    if depth == 0 && item.value == 1 && usages.first() == Some(&((PAGE_GENERIC_DESKTOP << 16) | USAGE_MULTI_AXIS_CONTROLLER)) {
                        summary.multi_axis_controller = true;
                        controller = Some(depth);
                    }
                    depth = depth.saturating_add(1);
                }
                (0, 0xC) => {
                    depth = depth.saturating_sub(1);
                    if controller == Some(depth) {
                        controller = None;
                    }
                }
                _ => {}
            }
            if item.kind == 0 {
                usages.clear();
                usage_min = None;
            }
        }
        summary
    }

    /// Records the axes and buttons among an input item's fields: the `i`th field takes the `i`th
    /// usage, and fields past the last usage take the last one (HID 1.11, 6.2.2.8). The first
    /// declaration of an axis or button wins.
    fn note_fields(&mut self, globals: &Globals, usages: &[u32], start: u32, relative: bool) {
        let bits = globals.report_size;
        if !(1..=32).contains(&bits) {
            return;
        }
        for index in 0..globals.report_count.min(MAX_USAGES_PER_ITEM) {
            let Some(usage) = usages.get(index as usize).or(usages.last()) else { return };
            let Some(bit_offset) = index.checked_mul(bits).and_then(|offset| offset.checked_add(start)) else { return };
            if bit_offset.saturating_add(bits) > MAX_REPORT_BITS {
                return;
            }
            let axis = usage.wrapping_sub((PAGE_GENERIC_DESKTOP << 16) | FIRST_AXIS_USAGE) as usize;
            // Button usages start at 1; usage 0 on the page means "no button".
            let button = usage.wrapping_sub((PAGE_BUTTON << 16) | 1) as usize;
            let slot = match self.axes.get_mut(axis) {
                Some(slot) => Some(slot),
                None => self.buttons.get_mut(button),
            };
            if let Some(slot) = slot
                && slot.is_none()
            {
                *slot = Some(Field {
                    report_id: globals.report_id,
                    bit_offset,
                    bits,
                    logical_min: globals.logical_min,
                    logical_max: globals.logical_max(),
                    relative,
                });
            }
        }
    }

    /// True for a top-level Generic Desktop "Multi-Axis Controller" collection. This tells a
    /// 3D mouse's motion interface apart from its keyboard interface and from ordinary mice,
    /// whoever made it.
    pub fn is_multi_axis_controller(&self) -> bool {
        self.multi_axis_controller
    }

    /// Where the device's axes and buttons are, or None when the descriptor names no motion axis:
    /// a controller with only buttons isn't something to steer the view with, and guessing a
    /// layout for it would read its reports as motion.
    ///
    /// In a descriptor that numbers its reports, fields declared before the first report id carry
    /// id 0, which no report can have, so they're left out.
    pub fn layout(&self) -> Option<Layout> {
        let keep = |field: &Option<Field>| field.filter(|field| !self.numbered || field.report_id != 0);
        let axes = self.axes.each_ref().map(keep);
        axes.iter().any(Option::is_some).then(|| Layout { axes, buttons: self.buttons.each_ref().map(keep), numbered: self.numbered })
    }
}

/// Usage items of up to two bytes name a usage on the current page, four bytes carry the page.
fn full_usage(page: u32, item: Item) -> u32 {
    if item.size == 4 { item.value } else { (page << 16) | item.value }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Report 1 = X Y Z, report 2 = Rx Ry Rz, each as three signed 16-bit values (3Dconnexion's
    /// wired devices).
    const SPLIT: &[u8] = &[
        0x05, 0x01, // usage page: generic desktop
        0x09, 0x08, // usage: multi-axis controller
        0xA1, 0x01, // collection: application
        0xA1, 0x00, // collection: physical
        0x85, 0x01, // report id 1
        0x16, 0xA2, 0xFE, // logical minimum -350
        0x26, 0x5E, 0x01, // logical maximum 350
        0x09, 0x30, 0x09, 0x31, 0x09, 0x32, // X Y Z
        0x75, 0x10, // report size 16
        0x95, 0x03, // report count 3
        0x81, 0x06, // input: data, variable, relative
        0xC0, // end collection
        0xA1, 0x00, // collection: physical
        0x85, 0x02, // report id 2
        0x09, 0x33, 0x09, 0x34, 0x09, 0x35, // Rx Ry Rz
        0x75, 0x10, 0x95, 0x03, 0x81, 0x06, // 3 x 16 bit, relative
        0xC0, // end collection
        0xA1, 0x02, // collection: logical
        0x85, 0x03, // report id 3
        0x05, 0x09, 0x19, 0x01, 0x29, 0x0F, // buttons 1 to 15
        0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x0F, 0x81, 0x02, // 15 x 1 bit, absolute
        0x95, 0x01, 0x81, 0x03, // 1 bit of padding
        0xC0, 0xC0,
    ];

    /// Report 1 = X Y Z Rx Ry Rz, absolute (3Dconnexion's wireless devices).
    const COMBINED: &[u8] = &[
        0x05, 0x01, 0x09, 0x08, 0xA1, 0x01, 0xA1, 0x00, 0x85, 0x01, // multi-axis controller, report id 1
        0x16, 0xA2, 0xFE, 0x26, 0x5E, 0x01, // -350..350
        0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x33, 0x09, 0x34, 0x09, 0x35, // X Y Z Rx Ry Rz
        0x75, 0x10, 0x95, 0x06, 0x81, 0x02, // 6 x 16 bit, absolute
        0xC0, 0xC0,
    ];

    /// The same six axes declared as a usage range, with report id 1 after the collection opens.
    const COMBINED_RANGE: &[u8] = &[
        0x05, 0x01, 0x09, 0x08, 0xA1, 0x01, 0x85, 0x01, 0x75, 0x10, 0x95, 0x06, //
        0x16, 0xA2, 0xFE, 0x26, 0x5E, 0x01, //
        0x19, 0x30, 0x29, 0x35, // usage minimum X, usage maximum Rz
        0x81, 0x06, 0xC0,
    ];

    /// A homemade trackball: report 1 is eight button bits, then six 16-bit relative axes over
    /// the full signed range.
    const BUTTONS_FIRST: &[u8] = &[
        0x05, 0x01, 0x09, 0x08, 0xA1, 0x01, 0x85, 0x01, // multi-axis controller, report id 1
        0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, // buttons 1 to 8, 0..1
        0x75, 0x01, 0x95, 0x08, 0x81, 0x02, // 8 x 1 bit
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x33, 0x09, 0x34, 0x09, 0x35, // X..Rz
        0x16, 0x00, 0x80, 0x26, 0xFF, 0x7F, // -32768..32767
        0x75, 0x10, 0x95, 0x06, 0x81, 0x06, // 6 x 16 bit, relative
        0xC0,
    ];

    /// A homemade joystick without report ids: four 8-bit axes from 0 to 255, a byte of constant
    /// padding first, and the rotation pushed and popped around a narrower range.
    const UNNUMBERED_UNSIGNED: &[u8] = &[
        0x05, 0x01, 0x09, 0x08, 0xA1, 0x01, //
        0x75, 0x08, 0x95, 0x01, 0x81, 0x03, // one constant byte
        0x15, 0x00, 0x26, 0xFF, 0x00, // 0..255
        0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x95, 0x03, 0x81, 0x02, // X Y Z
        0xA4, // push
        0x26, 0x7F, 0x00, // 0..127
        0x09, 0x35, 0x95, 0x01, 0x81, 0x02, // Rz
        0xB4, // pop
        0xC0,
    ];

    /// A boot keyboard-like interface: usage page 7 usages must not count as axes.
    const KEYBOARD: &[u8] =
        &[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0xC0];

    /// An ordinary mouse: Generic Desktop X, Y but top-level usage is Mouse (2).
    const MOUSE: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81,
        0x06, 0xC0, 0xC0,
    ];

    fn field(report_id: u8, bit_offset: u32, bits: u32, (logical_min, logical_max): (i64, i64), relative: bool) -> Option<Field> {
        Some(Field { report_id, bit_offset, bits, logical_min, logical_max, relative })
    }

    fn layout(bytes: &[u8]) -> Layout {
        Descriptor::parse(bytes).layout().expect("the descriptor declares motion axes")
    }

    /// One report's axes, X to Rz, from a list of (axis, value).
    fn axes(values: &[(usize, i16)]) -> [Option<i16>; AXIS_COUNT] {
        let mut axes = [None; AXIS_COUNT];
        for &(axis, value) in values {
            if let Some(slot) = axes.get_mut(axis) {
                *slot = Some(value);
            }
        }
        axes
    }

    /// A Multi-axis Controller application collection around `body`.
    fn controller(body: &[u8]) -> Vec<u8> {
        [&[0x05, 0x01, 0x09, 0x08, 0xA1, 0x01][..], body, &[0xC0]].concat()
    }

    #[test]
    fn split_reports_put_translation_in_report_one_and_rotation_in_report_two() {
        let descriptor = Descriptor::parse(SPLIT);
        assert!(descriptor.is_multi_axis_controller());
        let layout = layout(SPLIT);
        assert!(layout.numbered);
        let r = (-350, 350);
        assert_eq!(
            layout.axes,
            [
                field(1, 0, 16, r, true),
                field(1, 16, 16, r, true),
                field(1, 32, 16, r, true),
                field(2, 0, 16, r, true),
                field(2, 16, 16, r, true),
                field(2, 32, 16, r, true)
            ]
        );
        // The same axes as are assumed for a 3Dconnexion device whose descriptor can't be read.
        assert_eq!(layout.axes, Layout::default().axes);
        // Buttons 1 to 15 are report 3's first 15 bits; the 16th is padding.
        assert_eq!(layout.buttons[0], field(3, 0, 1, (0, 1), false));
        assert_eq!(layout.buttons[14], field(3, 14, 1, (0, 1), false));
        assert_eq!(layout.buttons[15], None);
        // Buttons 1, 3 and 15 down; then all up, which is a report too.
        assert_eq!(layout.decode(&[3, 0b101, 0b0100_0000]), Some(Report { axes: [None; 6], buttons: Some(0b101 | 1 << 14) }));
        assert_eq!(layout.decode(&[3, 0, 0]).map(|r| r.buttons), Some(Some(0)));
        // A motion report says nothing about the buttons.
        assert_eq!(layout.decode(&[1, 1, 0, 2, 0, 3, 0]).map(|r| r.buttons), Some(None));
    }

    #[test]
    fn buttons_keep_their_own_numbers_across_gaps() {
        // X in report 1; buttons 1 to 3, a constant bit, then buttons 5 and 6 in report 3, as
        // 3Dconnexion numbers them.
        let layout = layout(&controller(&[
            0x85, 0x01, 0x16, 0xA2, 0xFE, 0x26, 0x5E, 0x01, 0x75, 0x10, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02, // report 1: X
            0x85, 0x03, 0x05, 0x09, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, // report 3, 1-bit fields
            0x19, 0x01, 0x29, 0x03, 0x95, 0x03, 0x81, 0x02, // buttons 1 to 3
            0x95, 0x01, 0x81, 0x03, // a constant bit
            0x19, 0x05, 0x29, 0x06, 0x95, 0x02, 0x81, 0x02, // buttons 5 and 6
        ]));
        assert_eq!(layout.buttons[3], None);
        assert_eq!(layout.buttons[4], field(3, 4, 1, (0, 1), false));
        assert_eq!(layout.buttons[5], field(3, 5, 1, (0, 1), false));
        // Bits 0 and 5 set: buttons 1 and 6.
        assert_eq!(layout.decode(&[3, 0b10_0001]).and_then(|r| r.buttons), Some(0b10_0001));
    }

    #[test]
    fn a_controller_without_motion_axes_has_no_layout() {
        // Buttons only, in report 1. With a guessed 3Dconnexion layout, its report
        // `01 01 00 00 00 00 00` would read as X = 1. It must not be read at all.
        let buttons_only = controller(&[
            0x85, 0x01, 0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, //
            0x75, 0x08, 0x95, 0x05, 0x81, 0x03, // padding up to 6 bytes
        ]);
        assert!(Descriptor::parse(&buttons_only).is_multi_axis_controller());
        assert_eq!(Descriptor::parse(&buttons_only).layout(), None);
        // Axes wider than 32 bits aren't supported, so they don't count either.
        let wide = controller(&[0x75, 0x40, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02]);
        assert_eq!(Descriptor::parse(&wide).layout(), None);
        // A page-7 usage that happens to be 0x30 isn't an axis.
        let four_byte = controller(&[0x85, 0x01, 0x75, 0x10, 0x95, 0x01, 0x0B, 0x30, 0x00, 0x07, 0x00, 0x81, 0x06]);
        assert_eq!(Descriptor::parse(&four_byte).layout(), None);
    }

    #[test]
    fn six_axes_in_report_one_follow_each_other() {
        for (bytes, relative) in [(COMBINED, false), (COMBINED_RANGE, true)] {
            assert!(Descriptor::parse(bytes).is_multi_axis_controller());
            for (index, axis) in layout(bytes).axes.iter().enumerate() {
                assert_eq!(*axis, field(1, 16 * index as u32, 16, (-350, 350), relative), "axis {index}");
            }
        }
    }

    #[test]
    fn axes_after_the_buttons_start_after_the_buttons() {
        let layout = layout(BUTTONS_FIRST);
        assert_eq!(layout.axes[0], field(1, 8, 16, (-32768, 32767), true));
        assert_eq!(layout.axes[5], field(1, 8 + 5 * 16, 16, (-32768, 32767), true));
        // A report: buttons 0b101, then X = -100, Rz = 32767; the rest 0.
        let mut report = vec![1, 0b101];
        report.extend((-100_i16).to_le_bytes());
        report.extend([0; 8]);
        report.extend(32767_i16.to_le_bytes());
        let decoded = layout.decode(&report).expect("report 1 carries the axes");
        assert_eq!(decoded.axes, axes(&[(0, -1), (1, 0), (2, 0), (3, 0), (4, 0), (5, 350)]));
        // The same report carries buttons 1 to 8: 1 and 3 are down.
        assert_eq!(decoded.buttons, Some(0b101));
    }

    #[test]
    fn unnumbered_unsigned_axes_after_padding_are_centred() {
        let layout = layout(UNNUMBERED_UNSIGNED);
        assert!(!layout.numbered);
        assert_eq!(layout.axes[0], field(0, 8, 8, (0, 255), false));
        assert_eq!(layout.axes[2], field(0, 24, 8, (0, 255), false));
        assert_eq!(layout.axes[3], None);
        // PUSH and POP: Rz has its own range, and the size and count it was declared with.
        assert_eq!(layout.axes[5], field(0, 32, 8, (0, 127), false));
        // No id byte: padding, X at its minimum, Y at the top, Z near the middle, Rz at the top.
        let decoded = layout.decode(&[0xEE, 0, 255, 128, 127]).expect("one report carries everything");
        assert_eq!(decoded.axes, axes(&[(0, -350), (1, 350), (2, 1), (5, 350)]));
    }

    #[test]
    fn other_interfaces_are_not_3d_mice() {
        for bytes in [KEYBOARD, MOUSE, &[][..]] {
            let descriptor = Descriptor::parse(bytes);
            assert!(!descriptor.is_multi_axis_controller());
            assert_eq!(descriptor.layout(), None);
        }
        // A mouse's X and Y aren't axes, even though the generic desktop usages are the same.
        assert!(Descriptor::parse(MOUSE).axes.iter().all(Option::is_none));
    }

    #[test]
    fn an_end_collection_closes_the_controller() {
        // X Y Z in the controller; Rx in a top-level Mouse collection after it isn't the
        // controller's.
        let bytes =
            [
                &controller(&[
                    0x85, 0x01, 0x16, 0xA2, 0xFE, 0x26, 0x5E, 0x01, 0x75, 0x10, 0x95, 0x03, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x81, 0x02,
                ])[..],
                &[0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x33, 0x75, 0x10, 0x95, 0x01, 0x81, 0x02, 0xC0],
            ]
            .concat();
        let layout = layout(&bytes);
        assert!(layout.axes[0].is_some());
        assert_eq!(layout.axes[3], None);
    }

    #[test]
    fn decodes_separate_reports_and_ignores_short_ones_and_padding() {
        let layout = Layout::default();
        assert_eq!(layout.decode(&[1, 1, 2]), None);
        assert_eq!(layout.decode(&[]), None);
        assert_eq!(layout.decode(&[3, 1, 0, 0, 0, 0, 0]), None);
        assert_eq!(layout.decode(&[1, 0x9c, 0xff, 0x2c, 0x01, 0, 0]), Some(Report { axes: axes(&[(0, -100), (1, 300), (2, 0)]), buttons: None }));
        assert_eq!(layout.decode(&[2, 0, 0, 1, 0, 0xff, 0xff]), Some(Report { axes: axes(&[(3, 0), (4, 1), (5, -1)]), buttons: None }));
        // A padded translation report: its padding is not rotation.
        assert_eq!(layout.decode(&[1, 1, 0, 2, 0, 3, 0, 9, 9, 9, 9, 9, 9]), Some(Report { axes: axes(&[(0, 1), (1, 2), (2, 3)]), buttons: None }));
    }

    #[test]
    fn combined_reports_carry_both_groups() {
        let layout = layout(COMBINED);
        let report = [1, 0x9c, 0xff, 0x2c, 0x01, 0, 0, 1, 0, 0, 0, 0xff, 0xff, 0, 0];
        assert_eq!(layout.decode(&report).map(|r| r.axes), Some(axes(&[(0, -100), (1, 300), (2, 0), (3, 1), (4, 0), (5, -1)])));
        // One byte short of Rz: dropped whole rather than half read.
        assert_eq!(layout.decode(&report[..12]), None);
    }

    #[test]
    fn a_group_spread_over_reports_reports_only_what_each_carries() {
        // X in report 1, Y in report 2: each report says nothing about the other axis.
        let layout = layout(&controller(&[
            0x16, 0xA2, 0xFE, 0x26, 0x5E, 0x01, 0x75, 0x10, 0x95, 0x01, //
            0x85, 0x01, 0x09, 0x30, 0x81, 0x02, // report 1: X
            0x85, 0x02, 0x09, 0x31, 0x81, 0x02, // report 2: Y
        ]));
        assert_eq!(layout.decode(&[1, 100, 0]).map(|r| r.axes), Some(axes(&[(0, 100)])));
        assert_eq!(layout.decode(&[2, 200, 0]).map(|r| r.axes), Some(axes(&[(1, 200)])));
    }

    #[test]
    fn a_short_report_is_dropped_whole_axes_included() {
        // Report 1: X (16 bits), then a button. A report one byte short has X but not the button:
        // it's damaged, so neither is taken.
        let layout = layout(&controller(&[
            0x85, 0x01, 0x16, 0xA2, 0xFE, 0x26, 0x5E, 0x01, 0x75, 0x10, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02, // X
            0x05, 0x09, 0x09, 0x01, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x81, 0x02, 0x75, 0x07, 0x81, 0x03, // button 1, padding
        ]));
        assert_eq!(layout.decode(&[1, 100, 0]), None);
        assert_eq!(layout.decode(&[1, 100, 0, 1]), Some(Report { axes: axes(&[(0, 100)]), buttons: Some(1) }));
    }

    #[test]
    fn fields_declared_before_the_first_report_id_are_left_out() {
        // X before any report id, then Y in report 1. In a numbered descriptor X would have to be
        // in a report with id 0, which can't exist.
        let layout = layout(&controller(&[
            0x16, 0xA2, 0xFE, 0x26, 0x5E, 0x01, 0x75, 0x10, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02, // X, no id yet
            0x85, 0x01, 0x09, 0x31, 0x81, 0x02, // report 1: Y
        ]));
        assert_eq!(layout.axes[0], None);
        assert_eq!(layout.axes[1], field(1, 0, 16, (-350, 350), false));
        assert_eq!(layout.decode(&[0, 100, 0]), None);
        assert_eq!(layout.decode(&[1, 100, 0]).map(|r| r.axes), Some(axes(&[(1, 100)])));
    }

    #[test]
    fn push_and_pop_restore_the_globals_however_deep() {
        // PUSH, a 4-bit constant field, POP: X then takes the pushed 8-bit size, so it starts at
        // bit 4 and is 8 bits wide.
        let pushed = layout(&controller(&[
            0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x01, // 0..255, 8 bits, 1 field
            0xA4, 0x75, 0x04, 0x81, 0x03, 0xB4, // push, 4 constant bits, pop
            0x09, 0x30, 0x81, 0x02, // X
        ]));
        assert_eq!(pushed.axes[0], field(0, 4, 8, (0, 255), false));
        assert_eq!(pushed.decode(&[0xF0, 0x0F]).map(|r| r.axes), Some(axes(&[(0, 350)])));
        // 20 PUSHes, the Nth saving the size before it and then setting the size to N. Every POP
        // restores what its own PUSH saved: the first POP gives back 19, the 20th gives back 8.
        let mut deep = vec![0x15, 0x00, 0x25, 0x01, 0x95, 0x01, 0x75, 0x08];
        for depth in 1..=20_u8 {
            deep.extend([0xA4, 0x75, depth]);
        }
        deep.extend([0x81, 0x03]); // 20 constant bits at the deepest size
        deep.extend([0xB4, 0x09, 0x31, 0x81, 0x02]); // one POP: Y is 19 bits
        deep.extend([0xB4; 19]);
        deep.extend([0x09, 0x30, 0x81, 0x02]); // 19 more POPs: X is 8 bits
        let deep = layout(&controller(&deep));
        assert_eq!(deep.axes[1].map(|f| (f.bit_offset, f.bits)), Some((20, 19)));
        assert_eq!(deep.axes[0].map(|f| (f.bit_offset, f.bits)), Some((39, 8)));
    }

    #[test]
    fn a_long_item_is_skipped_whole() {
        // A long item (prefix 0xFE, 3 data bytes) in front changes nothing.
        let with_long_item = [&[0xFE, 0x03, 0x00, 0x01, 0x02, 0x03][..], COMBINED].concat();
        assert_eq!(layout(&with_long_item), layout(COMBINED));
    }

    #[test]
    fn negative_and_one_byte_ranges_keep_their_signs() {
        // -127..127 in one-byte items: 0x81 is -127, the bottom of the range.
        let one_byte = layout(&controller(&[0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02]));
        assert_eq!(one_byte.axes[0].map(|f| (f.logical_min, f.logical_max)), Some((-127, 127)));
        assert_eq!(one_byte.decode(&[0x81]).map(|r| r.axes), Some(axes(&[(0, -350)])));
        // -128..-1: a negative maximum stays negative, so 0xFF (-1) is the top.
        let negative = layout(&controller(&[0x15, 0x80, 0x25, 0xFF, 0x75, 0x08, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02]));
        assert_eq!(negative.axes[0].map(|f| f.logical_max), Some(-1));
        assert_eq!(negative.decode(&[0xFF]).map(|r| r.axes), Some(axes(&[(0, 350)])));
    }

    #[test]
    fn ranges_scale_to_full_scale() {
        let f = |min, max, bits| Field { report_id: 0, bit_offset: 0, bits, logical_min: min, logical_max: max, relative: false };
        assert_eq!(f(-350, 350, 16).scaled(-350), -350);
        assert_eq!(f(-350, 350, 16).scaled(123), 123);
        assert_eq!(f(-32768, 32767, 16).scaled(32767), 350);
        assert_eq!(f(-32768, 32767, 16).scaled(-32768), -350);
        assert_eq!(f(0, 1023, 10).scaled(0), -350);
        assert_eq!(f(0, 1023, 10).scaled(1023), 350);
        // No usable range: passed through, clamped to 16 bits.
        assert_eq!(f(0, 0, 32).scaled(40), 40);
        assert_eq!(f(5, 5, 32).scaled(1 << 20), i16::MAX);
        // Past the declared range: beyond ±350, but never i16::MIN, whose negation overflows.
        assert_eq!(f(-350, 350, 32).scaled(700), 700);
        assert_eq!(f(-350, 350, 32).scaled(-(1 << 30)), -i16::MAX);
        assert_eq!(f(5, 5, 32).scaled(-(1 << 20)), -i16::MAX);
        // Unsigned maxima: with a minimum of 0, 0x26 0xFF 0xFF is 65535 and the 4-byte
        // 0x27 0xFF 0xFF 0xFF 0xFF is 4294967295, so full deflection of a 32-bit field is 350.
        let two_byte = layout(&controller(&[0x15, 0x00, 0x26, 0xFF, 0xFF, 0x75, 0x10, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02]));
        assert_eq!(two_byte.axes[0].map(|f| f.logical_max), Some(65535));
        assert_eq!(two_byte.decode(&[0xFF, 0xFF]).map(|r| r.axes), Some(axes(&[(0, 350)])));
        let four_byte = layout(&controller(&[0x15, 0x00, 0x27, 0xFF, 0xFF, 0xFF, 0xFF, 0x75, 0x20, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02]));
        assert_eq!(four_byte.axes[0].map(|f| f.logical_max), Some(4_294_967_295));
        assert_eq!(four_byte.decode(&[0xFF; 4]).map(|r| r.axes), Some(axes(&[(0, 350)])));
    }

    #[test]
    fn a_field_must_be_1_to_32_bits_wide() {
        // Fields are public, so a hand-built one can be any width; reading it mustn't panic.
        for bits in [0, 33, 64, 65, u32::MAX] {
            let layout =
                Layout { axes: [field(0, 0, bits, (-1, 1), false), None, None, None, None, None], buttons: [None; MAX_BUTTONS], numbered: false };
            assert_eq!(layout.decode(&[0; 16]), None, "{bits} bits");
        }
        // Nor can any range: the extremes of i64 don't overflow the scaling.
        for (min, max) in [(i64::MIN, i64::MAX), (i64::MIN, 0), (0, i64::MAX), (i64::MAX, i64::MIN)] {
            for (bits, report) in [(1, [0xFF; 4]), (32, [0xFF; 4]), (32, [0; 4])] {
                let layout = Layout {
                    axes: [field(0, 0, bits, (min, max), false), None, None, None, None, None],
                    buttons: [None; MAX_BUTTONS],
                    numbered: false,
                };
                assert!(layout.decode(&report).is_some(), "{min}..{max}, {bits} bits");
            }
        }
    }

    #[test]
    fn fields_reach_the_end_of_the_biggest_report_and_no_further() {
        // An 8-bit X after 514 bytes of padding is still inside a report.
        let far = layout(&controller(&[0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x96, 0x02, 0x02, 0x81, 0x03, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02]));
        assert_eq!(far.axes[0].map(|f| f.bit_offset), Some(514 * 8));
        let mut report = vec![0; 515];
        if let Some(last) = report.last_mut() {
            *last = 0xFF;
        }
        assert_eq!(far.decode(&report).map(|r| r.axes), Some(axes(&[(0, 350)])));
        // A 32-bit X that ends exactly at the last bit is kept; one byte later it's outside.
        let at_end = |padding: u16| {
            let [low, high] = padding.to_le_bytes();
            Descriptor::parse(&controller(&[0x75, 0x08, 0x96, low, high, 0x81, 0x03, 0x75, 0x20, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02])).layout()
        };
        assert_eq!(at_end(4092).and_then(|l| l.axes[0]).map(|f| f.bit_offset), Some(4092 * 8));
        assert_eq!(at_end(4093), None);
    }

    #[test]
    fn truncated_and_hostile_input_never_panics() {
        for bytes in [SPLIT, COMBINED, BUTTONS_FIRST, UNNUMBERED_UNSIGNED] {
            for end in 0..=bytes.len() {
                if let Some(layout) = Descriptor::parse(&bytes[..end]).layout() {
                    let _ = layout.decode(&[1, 2, 3]);
                }
            }
        }
        // Long item with a length running past the end, huge usage ranges and counts, deep nesting,
        // pops without pushes, pushes without pops, 32-bit fields at the end of the biggest report.
        let _ = Descriptor::parse(&[0xFE, 0xFF, 0x00, 0x01]);
        let _ = Descriptor::parse(&[0x05, 0x01, 0x1B, 0, 0, 0, 0, 0x2B, 0xFF, 0xFF, 0xFF, 0xFF, 0x81, 0x06]);
        let _ = Descriptor::parse(&controller(&[0x77, 0xFF, 0xFF, 0xFF, 0xFF, 0x97, 0xFF, 0xFF, 0xFF, 0xFF, 0x09, 0x30, 0x81, 0x02]));
        let _ = Descriptor::parse(&controller(&[0x75, 0x20, 0x96, 0xFF, 0x03, 0x09, 0x30, 0x81, 0x02, 0x09, 0x31, 0x81, 0x02]));
        let _ = Descriptor::parse(&[0xC0; 64]);
        let _ = Descriptor::parse(&[0xA1; 64]);
        let _ = Descriptor::parse(&[0xB4; 64]);
        let _ = Descriptor::parse(&[0xA4; MAX_DESCRIPTOR_BYTES * 2]);
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        };
        for _ in 0..500 {
            let bytes: Vec<u8> = [0x05, 0x01, 0x09, 0x08, 0xA1, 0x01].into_iter().chain((0..64).map(|_| next())).collect();
            let report: Vec<u8> = (0..16).map(|_| next()).collect();
            if let Some(layout) = Descriptor::parse(&bytes).layout() {
                let _ = layout.decode(&report);
            }
        }
    }
}

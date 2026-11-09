//! Big5-UAO 2.50, the encoding PTT speaks.

static B2U: &[u8] = include_bytes!("../data/b2u.bin");
static U2B: &[u8] = include_bytes!("../data/u2b.bin");

const LEAD_MIN: u8 = 0x81;
const LEAD_MAX: u8 = 0xFE;
const TRAIL_MIN: u8 = 0x40;
const TRAIL_MAX: u8 = 0xFE;
const TRAILS: usize = (TRAIL_MAX - TRAIL_MIN) as usize + 1;

pub fn is_lead(byte: u8) -> bool {
    (LEAD_MIN..=LEAD_MAX).contains(&byte)
}

fn word(table: &[u8], index: usize) -> u16 {
    u16::from_le_bytes([table[index * 2], table[index * 2 + 1]])
}

pub fn decode(lead: u8, trail: u8) -> Option<char> {
    if !is_lead(lead) || !(TRAIL_MIN..=TRAIL_MAX).contains(&trail) {
        return None;
    }
    let index = (lead - LEAD_MIN) as usize * TRAILS + (trail - TRAIL_MIN) as usize;
    match word(B2U, index) {
        0 => None,
        code => char::from_u32(code.into()),
    }
}

pub fn encode(c: char) -> Option<[u8; 2]> {
    let code = u32::from(c);
    if code > 0xFFFF {
        return None;
    }
    match word(U2B, code as usize) {
        0 => None,
        big5 => Some(big5.to_be_bytes()),
    }
}

/// Appends `text` as Big5-UAO. Characters outside the table become `?`.
pub fn encode_str(text: &str, out: &mut Vec<u8>) {
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c as u8);
        } else if let Some(pair) = encode(c) {
            out.extend_from_slice(&pair);
        } else {
            out.push(b'?');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_common_text() {
        for c in "批踢踢實業坊推噓鄉民─│█▄▌◆※".chars() {
            let [lead, trail] = encode(c).unwrap_or_else(|| panic!("{c} has no mapping"));
            assert_eq!(decode(lead, trail), Some(c));
        }
    }

    #[test]
    fn decodes_uao_extensions() {
        // Kana and the 喆 family only exist in the UAO extension rows.
        assert_eq!(encode('あ'), Some([0xC6, 0xE8]));
        assert_eq!(encode('喆'), Some([0x95, 0xED]));
        assert_eq!(decode(0x95, 0xED), Some('喆'));
    }

    #[test]
    fn rejects_bytes_outside_the_table() {
        assert_eq!(decode(0x80, 0x40), None);
        assert_eq!(decode(0xA4, 0x3F), None);
        assert_eq!(decode(0xA4, 0xFF), None);
        assert_eq!(decode(b'a', b'b'), None);
    }

    #[test]
    fn unmappable_text_degrades_to_question_marks() {
        let mut out = Vec::new();
        encode_str("a😀推", &mut out);
        assert_eq!(out, [b'a', b'?', 0xB1, 0xC0]);
    }
}
